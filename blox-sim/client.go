package main

import (
	"bufio"
	"fmt"
	"net"
	"strconv"
	"strings"
	"sync"
	"time"
)

// Client is one connection to blox-server.
//
// Go talks to the engine over a socket rather than linking it. That is the
// deliberate choice from DESIGN.md D5/D7: cgo costs ~50-100ns per call and
// fights the scheduler, so a Go process that needs the engine should not
// embed it. Everything below therefore measures the *whole* path — socket,
// protocol, engine — which is the number that actually matters to a caller.
type Client struct {
	conn net.Conn
	w    *bufio.Writer
	r    *bufio.Reader

	mu     sync.Mutex
	nextID uint64
}

func Dial(addr string, idBase uint64) (*Client, error) {
	c, err := net.Dial("tcp", addr)
	if err != nil {
		return nil, err
	}
	if tc, ok := c.(*net.TCPConn); ok {
		_ = tc.SetNoDelay(true)
	}
	return &Client{
		conn:   c,
		w:      bufio.NewWriterSize(c, 1<<16),
		r:      bufio.NewReaderSize(c, 1<<16),
		nextID: idBase,
	}, nil
}

func (c *Client) Close() error { return c.conn.Close() }

// ID hands out order ids unique to this client, so concurrent agents never
// collide and trip DuplicateId.
func (c *Client) ID() uint64 {
	c.mu.Lock()
	defer c.mu.Unlock()
	c.nextID++
	return c.nextID
}

// Send queues a command. Nothing leaves the process until Flush.
func (c *Client) Send(format string, args ...any) error {
	if _, err := fmt.Fprintf(c.w, format, args...); err != nil {
		return err
	}
	return c.w.WriteByte('\n')
}

func (c *Client) Flush() error { return c.w.Flush() }

func (c *Client) ReadLine() (string, error) {
	s, err := c.r.ReadString('\n')
	return strings.TrimRight(s, "\r\n"), err
}

// Do sends one command and returns the single response line.
func (c *Client) Do(format string, args ...any) (string, error) {
	if err := c.Send(format, args...); err != nil {
		return "", err
	}
	if err := c.Flush(); err != nil {
		return "", err
	}
	return c.ReadLine()
}

// Ping is a barrier: it returns only once the engine has processed everything
// queued before it. Throughput measurement depends on this — without it you
// measure how fast the kernel accepts writes, not how fast the engine works.
func (c *Client) Ping(token string) error {
	if err := c.Send("PING %s", token); err != nil {
		return err
	}
	if err := c.Flush(); err != nil {
		return err
	}
	want := "PONG " + token
	deadline := time.Now().Add(60 * time.Second)
	for time.Now().Before(deadline) {
		line, err := c.ReadLine()
		if err != nil {
			return err
		}
		if line == want {
			return nil
		}
		// Anything else is an ack or a pushed event; keep draining.
	}
	return fmt.Errorf("ping %q timed out", token)
}

// Quiet suppresses per-command acks so pipelined throughput measures the
// engine rather than the acknowledgement traffic.
func (c *Client) Quiet(on bool) error {
	flag := "0"
	if on {
		flag = "1"
	}
	resp, err := c.Do("QUIET %s", flag)
	if err != nil {
		return err
	}
	if resp != "OK" {
		return fmt.Errorf("QUIET: %s", resp)
	}
	return nil
}

// Echo controls whether order-lifecycle pushes (fills, rejects) come back to
// this connection. Turn it off when pipelining without reading, or the
// server's bounded queue fills and it disconnects you as a slow consumer —
// which is correct behaviour, not a bug to work around.
func (c *Client) Echo(on bool) error {
	flag := "0"
	if on {
		flag = "1"
	}
	resp, err := c.Do("ECHO %s", flag)
	if err != nil {
		return err
	}
	if resp != "OK" {
		return fmt.Errorf("ECHO: %s", resp)
	}
	return nil
}

// Top is a parsed top-of-book. Missing sides are reported as absent rather
// than as zero — a zero price would silently look like a real level.
type Top struct {
	BidPx, BidQty  int64
	AskPx, AskQty  int64
	HasBid, HasAsk bool
}

func (t Top) Mid() int64 {
	if t.HasBid && t.HasAsk {
		return (t.BidPx + t.AskPx) / 2
	}
	if t.HasBid {
		return t.BidPx
	}
	return t.AskPx
}

func (t Top) Spread() (int64, bool) {
	if t.HasBid && t.HasAsk {
		return t.AskPx - t.BidPx, true
	}
	return 0, false
}

// parseTop reads "TOP <inst> <bidpx> <bidqty> <askpx> <askqty>" or the
// "EV TOP ..." push form. "-" marks an absent side.
func parseTop(fields []string) (Top, bool) {
	if len(fields) < 6 {
		return Top{}, false
	}
	var t Top
	if fields[2] != "-" {
		px, err1 := strconv.ParseInt(fields[2], 10, 64)
		qty, err2 := strconv.ParseInt(fields[3], 10, 64)
		if err1 == nil && err2 == nil {
			t.BidPx, t.BidQty, t.HasBid = px, qty, true
		}
	}
	if fields[4] != "-" {
		px, err1 := strconv.ParseInt(fields[4], 10, 64)
		qty, err2 := strconv.ParseInt(fields[5], 10, 64)
		if err1 == nil && err2 == nil {
			t.AskPx, t.AskQty, t.HasAsk = px, qty, true
		}
	}
	return t, true
}
