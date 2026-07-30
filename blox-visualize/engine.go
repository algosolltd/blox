// The engine bridge: one TCP client to blox-server, translating its
// newline-delimited protocol into values the hub can broadcast.
//
// The wire protocol gives subscribers trades and top-of-book pushes but has
// no aggregate-depth deltas, so depth is polled (`BOOK`) and conflated by
// the hub — D24's "coalesce, not sample": the browser always gets the
// freshest book, never a queue of stale ones.
//
// The engine is deliberately clockless (DESIGN.md D4), so nothing on the
// wire carries a timestamp. Every message is stamped here on receipt —
// recv_ts is the only honest time a consumer has (docs/ADAPTERS.md §1).
package main

import (
	"bufio"
	"context"
	"fmt"
	"net"
	"strconv"
	"strings"
	"time"
)

// Trade is one matched execution. Side is the aggressor: "B" lifted the
// offer, "S" hit the bid. Maker/Taker stay off the wire; they exist so the
// hub can flag the user's own prints (Mine).
type Trade struct {
	Ts    int64  `json:"ts"`
	Price int64  `json:"price"`
	Qty   int64  `json:"qty"`
	Side  string `json:"side"`
	Mine  bool   `json:"mine,omitempty"`
	Maker uint64 `json:"-"`
	Taker uint64 `json:"-"`
}

// Order-lifecycle events, echoed only to the connection that submitted the
// order — which is the bridge itself, so all of these are the web user's.
type Ack struct{ ID uint64 }

type Fill struct {
	ID        uint64
	Price     int64
	Qty       int64
	Remaining int64
	Ts        int64
}

type CancelEv struct {
	ID        uint64
	Remaining int64
	Ts        int64
}

// AmendEv is a resting order's quantity changing in place — the "partial
// delete" case is a decrease, which keeps the order's book priority.
type AmendEv struct {
	ID  uint64
	Qty int64
	Ts  int64
}

type RejectEv struct {
	ID     uint64
	Reason string
	Ts     int64
}

// Book is a full aggregate-depth snapshot, best first.
type Book struct {
	Ts   int64      `json:"ts"`
	Bids [][2]int64 `json:"bids"`
	Asks [][2]int64 `json:"asks"`
}

// Stats mirrors the server's STATS reply.
type Stats struct {
	Ts      int64  `json:"ts"`
	Applied uint64 `json:"applied"`
	Errors  uint64 `json:"errors"`
	Books   int    `json:"books"`
	Conns   int    `json:"conns"`
	Dropped uint64 `json:"dropped"`
}

// Status reports the bridge's own connection state, so the UI can tell a
// dead market from a dead bridge.
type Status struct {
	State  string `json:"state"` // "up" | "down"
	Detail string `json:"detail,omitempty"`
}

type Engine struct {
	Addr       string
	Instrument int
	Depth      int
	BookEvery  time.Duration
	StatsEvery time.Duration
	Out        chan<- any
	// In carries outbound lines (user orders, poll queries). Shared across
	// sessions: a reconnect picks up where the dead session left off —
	// except whatever it drained, which the account marks LOST.
	In chan string
}

// Run reconnects forever (adapter contract: backoff, capped) until the
// context is cancelled.
func (e *Engine) Run(ctx context.Context) {
	backoff := 300 * time.Millisecond
	for {
		if ctx.Err() != nil {
			return
		}
		err := e.session(ctx)
		if ctx.Err() != nil {
			return
		}
		e.emit(Status{State: "down", Detail: err.Error()})
		select {
		case <-ctx.Done():
			return
		case <-time.After(backoff):
		}
		if backoff < 2*time.Second {
			backoff *= 2
		}
	}
}

func (e *Engine) emit(v any) {
	select {
	case e.Out <- v:
	default: // hub overwhelmed; drop rather than block the reader
	}
}

func (e *Engine) session(ctx context.Context) error {
	conn, err := net.DialTimeout("tcp", e.Addr, 3*time.Second)
	if err != nil {
		return fmt.Errorf("dial %s: %w", e.Addr, err)
	}
	defer conn.Close()
	// Closing the socket on shutdown unblocks the read loop.
	go func() {
		<-ctx.Done()
		_ = conn.Close()
	}()

	// QUIET: no per-command OKs — the order lifecycle echo (EV ACK/REJECT)
	// tells us everything an OK would. ECHO 1: this connection is where the
	// web user's orders enter, and the echo is how the account tracks them.
	wr := bufio.NewWriter(conn)
	if _, err := fmt.Fprintf(wr, "QUIET 1\nECHO 1\nSUB %d\n", e.Instrument); err != nil {
		return err
	}
	if err := wr.Flush(); err != nil {
		return err
	}

	// A single goroutine owns all writes from here on.
	stop := make(chan struct{})
	defer func() {
		close(stop)
		// Anything not sent by this dead session must not be replayed by
		// the next one — the account has already marked those orders LOST.
		for {
			select {
			case <-e.In:
			default:
				return
			}
		}
	}()
	go func() {
		w := bufio.NewWriter(conn)
		for {
			select {
			case <-stop:
				return
			case line := <-e.In:
				if _, err := w.WriteString(line + "\n"); err != nil {
					return
				}
				if err := w.Flush(); err != nil {
					return
				}
			}
		}
	}()

	go func() {
		book := time.NewTicker(e.BookEvery)
		stats := time.NewTicker(e.StatsEvery)
		defer book.Stop()
		defer stats.Stop()
		for {
			select {
			case <-stop:
				return
			case <-book.C:
				select {
				case e.In <- fmt.Sprintf("BOOK %d %d", e.Instrument, e.Depth):
				default:
				}
			case <-stats.C:
				select {
				case e.In <- "STATS":
				default:
				}
			}
		}
	}()

	e.emit(Status{State: "up", Detail: e.Addr})

	r := bufio.NewReaderSize(conn, 1<<16)
	for {
		line, err := r.ReadString('\n')
		if err != nil {
			return fmt.Errorf("read: %w", err)
		}
		if msg := parseLine(strings.TrimSpace(line), time.Now().UnixMilli()); msg != nil {
			e.emit(msg)
		}
	}
}

// parseLine turns one wire line into a hub message, or nil if the line is
// not something the UI cares about (acks, provider-level pushes, ...).
func parseLine(line string, ts int64) any {
	f := strings.Fields(line)
	if len(f) == 0 {
		return nil
	}
	switch f[0] {
	case "EV":
		if len(f) < 3 {
			return nil
		}
		switch f[1] {
		case "TRADE":
			// EV TRADE <inst> <price> <qty> <maker> <taker> <B|S>
			if len(f) < 8 {
				return nil
			}
			price, err1 := strconv.ParseInt(f[3], 10, 64)
			qty, err2 := strconv.ParseInt(f[4], 10, 64)
			maker, err3 := strconv.ParseUint(f[5], 10, 64)
			taker, err4 := strconv.ParseUint(f[6], 10, 64)
			if err1 != nil || err2 != nil || err3 != nil || err4 != nil {
				return nil
			}
			return Trade{Ts: ts, Price: price, Qty: qty, Side: f[7], Maker: maker, Taker: taker}
		case "ACK":
			// EV ACK <id>
			if id, err := strconv.ParseUint(f[2], 10, 64); err == nil {
				return Ack{ID: id}
			}
		case "FILL":
			// EV FILL <id> <price> <qty> <remaining>
			if len(f) < 6 {
				return nil
			}
			id, err1 := strconv.ParseUint(f[2], 10, 64)
			price, err2 := strconv.ParseInt(f[3], 10, 64)
			qty, err3 := strconv.ParseInt(f[4], 10, 64)
			rem, err4 := strconv.ParseInt(f[5], 10, 64)
			if err1 == nil && err2 == nil && err3 == nil && err4 == nil {
				return Fill{ID: id, Price: price, Qty: qty, Remaining: rem, Ts: ts}
			}
		case "CANCEL":
			// EV CANCEL <id> <remaining>
			if len(f) < 4 {
				return nil
			}
			id, err1 := strconv.ParseUint(f[2], 10, 64)
			rem, err2 := strconv.ParseInt(f[3], 10, 64)
			if err1 == nil && err2 == nil {
				return CancelEv{ID: id, Remaining: rem, Ts: ts}
			}
		case "AMEND":
			// EV AMEND <id> <qty>
			if len(f) < 4 {
				return nil
			}
			id, err1 := strconv.ParseUint(f[2], 10, 64)
			qty, err2 := strconv.ParseInt(f[3], 10, 64)
			if err1 == nil && err2 == nil {
				return AmendEv{ID: id, Qty: qty, Ts: ts}
			}
		case "REJECT":
			// EV REJECT <id> <reason>
			if id, err := strconv.ParseUint(f[2], 10, 64); err == nil {
				reason := ""
				if len(f) > 3 {
					reason = f[3]
				}
				return RejectEv{ID: id, Reason: reason, Ts: ts}
			}
		}
	case "BOOK":
		// BOOK <inst> <bids> <asks>   (reply to our poll)
		if len(f) >= 4 {
			return Book{Ts: ts, Bids: parseLevels(f[2]), Asks: parseLevels(f[3])}
		}
	case "STATS":
		// STATS applied=N errors=N books=N conns=N dropped_deltas=N
		s := Stats{Ts: ts}
		for _, kv := range f[1:] {
			k, v, ok := strings.Cut(kv, "=")
			if !ok {
				continue
			}
			n, err := strconv.ParseUint(v, 10, 64)
			if err != nil {
				continue
			}
			switch k {
			case "applied":
				s.Applied = n
			case "errors":
				s.Errors = n
			case "books":
				s.Books = int(n)
			case "conns":
				s.Conns = int(n)
			case "dropped_deltas":
				s.Dropped = n
			}
		}
		return s
	}
	return nil
}

// parseLevels parses "10850:25,10849:100" (or "-" for an empty side).
func parseLevels(s string) [][2]int64 {
	if s == "-" || s == "" {
		return nil
	}
	parts := strings.Split(s, ",")
	out := make([][2]int64, 0, len(parts))
	for _, p := range parts {
		ps, qs, ok := strings.Cut(p, ":")
		if !ok {
			continue
		}
		price, err1 := strconv.ParseInt(ps, 10, 64)
		qty, err2 := strconv.ParseInt(qs, 10, 64)
		if err1 != nil || err2 != nil {
			continue
		}
		out = append(out, [2]int64{price, qty})
	}
	return out
}
