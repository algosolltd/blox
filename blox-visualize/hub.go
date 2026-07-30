// The hub: one goroutine that owns the client set and conflates the
// engine's event stream into 20 fps update frames (DESIGN.md D24 —
// coalesce, don't sample; D25 — a client that can't keep up is
// disconnected, never buffered without limit).
//
// It also fronts the user account: order and cancel messages arrive over
// the WebSocket, become wire lines for the engine, and the echoed lifecycle
// events come back through the event stream to mutate the ledger.
package main

import (
	"encoding/json"
	"fmt"
	"net/http"
	"time"
)

type wsClient struct {
	conn *wsConn
}

type Hub struct {
	register   chan *wsClient
	unregister chan *wsClient
	clients    map[*wsClient]struct{}

	hello      []byte // static config blob, built once
	lastBook   *Book
	lastStats  *Stats
	lastStatus []byte
	engineUp   bool

	account  *Account
	engineIn chan<- string

	flushEvery time.Duration
	maxBatch   int
}

func NewHub(name string, instrument, priceScale int, server string, account *Account, engineIn chan<- string) *Hub {
	hello, _ := json.Marshal(map[string]any{
		"type":       "hello",
		"name":       name,
		"instrument": instrument,
		"priceScale": priceScale,
		"server":     server,
	})
	return &Hub{
		register:   make(chan *wsClient),
		unregister: make(chan *wsClient),
		clients:    make(map[*wsClient]struct{}),
		hello:      hello,
		account:    account,
		engineIn:   engineIn,
		flushEvery: 50 * time.Millisecond,
		maxBatch:   4000,
	}
}

// updateMsg is the one frame the UI receives on the flush tick. Parts that
// didn't change since the last tick are simply absent.
type updateMsg struct {
	Type   string  `json:"type"`
	Trades []Trade `json:"trades,omitempty"`
	Book   *Book   `json:"book,omitempty"`
	Stats  *Stats  `json:"stats,omitempty"`
}

// accountMsg carries the user's orders and ledger. Full marks a structural
// snapshot (the client replaces its order tables, even with empty slices —
// omitempty would otherwise swallow "the last order just closed"); Pnl
// comes along whenever the mark moved.
type accountMsg struct {
	Type   string          `json:"type"`
	Full   bool            `json:"full,omitempty"`
	Open   []*AccountOrder `json:"open,omitempty"`
	Closed []*AccountOrder `json:"closed,omitempty"`
	Pnl    *PnL            `json:"pnl,omitempty"`
}

// noticeMsg is a transient toast — fills and rejects, never book noise.
type noticeMsg struct {
	Type  string `json:"type"` // "notice"
	Level string `json:"level"`
	Text  string `json:"text"`
}

type statusMsg struct {
	Type string `json:"type"` // "status"
	Status
}

func (h *Hub) Run(events <-chan any) {
	tick := time.NewTicker(h.flushEvery)
	defer tick.Stop()

	var trades []Trade
	var book *Book
	var stats *Stats

	for {
		select {
		case c := <-h.register:
			h.clients[c] = struct{}{}
			// Greet with config plus the freshest snapshots we hold, so a
			// new client paints a full UI on the first frames.
			h.push(c, h.hello)
			if h.lastBook != nil || h.lastStats != nil {
				if b, err := json.Marshal(updateMsg{Type: "update", Book: h.lastBook, Stats: h.lastStats}); err == nil {
					h.push(c, b)
				}
			}
			if h.lastStatus != nil {
				h.push(c, h.lastStatus)
			}
			h.push(c, h.fullAccount())

		case c := <-h.unregister:
			if _, ok := h.clients[c]; ok {
				delete(h.clients, c)
				c.conn.close()
			}

		case ev := <-events:
			switch m := ev.(type) {
			case Trade:
				m.Mine = h.account.IsUser(m.Maker, m.Taker)
				h.account.OnTrade(m.Price)
				trades = append(trades, m)
				if len(trades) > h.maxBatch { // keep the newest
					trades = trades[len(trades)-h.maxBatch:]
				}
			case Book:
				book = &m
				h.lastBook = book
				h.account.OnBook(m)
			case Stats:
				stats = &m
				h.lastStats = stats
			case Fill:
				if o := h.account.OnFill(m.ID, m.Price, m.Qty, m.Remaining, m.Ts); o != nil {
					h.notify("info", fmt.Sprintf("%s %s @ %s · filled %s%s",
						sideWord(o.Side), fmtQty(m.Qty), fmtPx(m.Price),
						fmtQty(o.Filled), leftNote(o)))
				}
			case CancelEv:
				h.account.OnCancel(m.ID, m.Remaining, m.Ts)
			case AmendEv:
				if o := h.account.OnAmend(m.ID, m.Qty, m.Ts); o != nil {
					h.notify("info", fmt.Sprintf("%s reduced to %s resting",
						sideWord(o.Side), fmtQty(m.Qty)))
				}
			case RejectEv:
				if o := h.account.OnReject(m.ID, m.Reason, m.Ts); o != nil {
					h.notify("warn", fmt.Sprintf("order rejected: %s", m.Reason))
				}
			case Ack:
				// The optimistic LIVE entry already shows the order.
			case Status:
				if m.State == "down" {
					h.engineUp = false
					h.account.OnEngineDown(time.Now().UnixMilli())
				} else {
					h.engineUp = true
				}
				b, _ := json.Marshal(statusMsg{Type: "status", Status: m})
				h.lastStatus = b
				h.broadcast(b) // state changes are rare — push immediately
			}

		case <-tick.C:
			structDirty, pnlDirty := h.account.TakeDirty()
			if structDirty || pnlDirty {
				msg := accountMsg{Type: "account", Full: structDirty}
				if structDirty {
					msg.Open, msg.Closed = h.account.SnapshotOrders()
				}
				pnl := h.account.SnapshotPnl()
				msg.Pnl = &pnl
				if b, err := json.Marshal(msg); err == nil {
					h.broadcast(b)
				}
			}
			if len(trades) == 0 && book == nil && stats == nil {
				continue
			}
			if b, err := json.Marshal(updateMsg{Type: "update", Trades: trades, Book: book, Stats: stats}); err == nil {
				h.broadcast(b)
			}
			trades = trades[:0]
			book = nil
			stats = nil
		}
	}
}

func (h *Hub) fullAccount() []byte {
	open, closed := h.account.SnapshotOrders()
	pnl := h.account.SnapshotPnl()
	b, _ := json.Marshal(accountMsg{Type: "account", Full: true, Open: open, Closed: closed, Pnl: &pnl})
	return b
}

func (h *Hub) notify(level, text string) {
	if b, err := json.Marshal(noticeMsg{Type: "notice", Level: level, Text: text}); err == nil {
		h.broadcast(b)
	}
}

func sideWord(s string) string {
	if s == "B" {
		return "BUY"
	}
	return "SELL"
}

func leftNote(o *AccountOrder) string {
	if o.Remaining > 0 && o.Status == stLive {
		return fmt.Sprintf(", %s resting", fmtQty(o.Remaining))
	}
	return ""
}

// Compact integer formatting for notifications (display scale lives in the
// UI, but toasts are built here where the values are).
func fmtPx(ticks int64) string {
	neg := ticks < 0
	if neg {
		ticks = -ticks
	}
	s := fmt.Sprintf("%d.%02d", ticks/100, ticks%100)
	if neg {
		return "-" + s
	}
	return s
}

func fmtQty(q int64) string {
	return fmt.Sprintf("%d", q)
}

func (h *Hub) broadcast(b []byte) {
	for c := range h.clients {
		h.push(c, b)
	}
}

func (h *Hub) push(c *wsClient, b []byte) {
	select {
	case c.conn.send <- wsFrame{op: opText, data: b}:
	default:
		// Slow consumer (D25): disconnect is recoverable, an unbounded
		// buffer is not.
		select {
		case h.unregister <- c:
		default:
		}
	}
}

// clientReq is everything the browser is allowed to ask for.
type clientReq struct {
	Type  string `json:"type"`
	Side  string `json:"side"`
	Kind  string `json:"kind"`
	Price int64  `json:"price"`
	Qty   int64  `json:"qty"`
	ID    uint64 `json:"id"`
}

// onClientMessage turns one inbound WebSocket message into at most one
// engine command. Rejections of *form* go back only to the asking client;
// rejections from the *engine* arrive as events and go to everyone.
func (h *Hub) onClientMessage(c *wsClient, data []byte) {
	var r clientReq
	if err := json.Unmarshal(data, &r); err != nil {
		return
	}
	switch r.Type {
	case "order":
		if !h.engineUp {
			h.push(c, mustJSON(noticeMsg{Type: "notice", Level: "err",
				Text: "engine disconnected — order not sent"}))
			return
		}
		_, wire, err := h.account.NewOrder(r.Side, r.Kind, r.Price, r.Qty, time.Now().UnixMilli())
		if err != nil {
			h.push(c, mustJSON(noticeMsg{Type: "notice", Level: "err", Text: err.Error()}))
			return
		}
		select {
		case h.engineIn <- wire:
		default:
			h.push(c, mustJSON(noticeMsg{Type: "notice", Level: "err", Text: "engine queue full"}))
		}
	case "cancel":
		wire, err := h.account.Cancel(r.ID)
		if err != nil {
			h.push(c, mustJSON(noticeMsg{Type: "notice", Level: "err", Text: err.Error()}))
			return
		}
		select {
		case h.engineIn <- wire:
		default:
		}
	case "reduce":
		wire, err := h.account.Reduce(r.ID, r.Qty)
		if err != nil {
			h.push(c, mustJSON(noticeMsg{Type: "notice", Level: "err", Text: err.Error()}))
			return
		}
		select {
		case h.engineIn <- wire:
		default:
		}
	}
}

func mustJSON(v any) []byte {
	b, _ := json.Marshal(v)
	return b
}

// ServeWS upgrades the connection and registers the client.
func (h *Hub) ServeWS(w http.ResponseWriter, r *http.Request) {
	conn, err := upgrade(w, r)
	if err != nil {
		http.Error(w, err.Error(), http.StatusBadRequest)
		return
	}
	c := &wsClient{conn: conn}
	go conn.writeLoop()
	go conn.readLoop(
		func(data []byte) { h.onClientMessage(c, data) },
		func() {
			select {
			case h.unregister <- c:
			default:
			}
		},
	)
	// Keepalive: dead TCP connections are detected by the read side only
	// when something arrives; pings make half-open sockets visible.
	go func() {
		t := time.NewTicker(30 * time.Second)
		defer t.Stop()
		for {
			select {
			case <-conn.done:
				return
			case <-t.C:
				conn.reply(opPing, nil)
			}
		}
	}()
	h.register <- c
}
