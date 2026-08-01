// The user account: orders the browser submitted, their lifecycle, and the
// resulting position — the four-number netting ledger from DESIGN.md
// (D20/D21), kept in integer ticks end to end.
//
// Everything here is derived from the order-lifecycle events the server
// echoes back on the bridge's own connection (ACK / FILL / CANCEL / REJECT).
// Those events only ever describe this connection's orders, so everything
// the account sees belongs to the web user — the sim's agents trade on
// their own connections and never touch this ledger.
package main

import (
	"fmt"
	"sort"
	"strings"
	"sync"
)

// Order-id namespace for the web user. blox-sim partitions the space in
// 1e9-wide spans up to 300e9; the web user sits far above them.
const (
	acctIDBase    = uint64(900_000_000_000)
	acctOwner     = uint64(999)
	acctClosedMax = 100
)

// Order status values.
const (
	stLive          = "LIVE"
	stFilled        = "FILLED"
	stCancelled     = "CANCELLED"
	stPartialCancel = "PARTIAL_CANCEL" // cancelled with some quantity already filled
	stRejected      = "REJECTED"
	stLost          = "LOST" // bridge dropped mid-flight; engine state unknown
)

type AccountOrder struct {
	ID        uint64 `json:"id"`
	Side      string `json:"side"`  // "B" | "S"
	Price     int64  `json:"price"` // limit price in ticks; 0 for market
	Qty       int64  `json:"qty"`
	Filled    int64  `json:"filled"`
	Remaining int64  `json:"remaining"`
	Kind      string `json:"kind"` // wire spelling: LIMIT MARKET IOC FOK POST
	Status    string `json:"status"`
	Reason    string `json:"reason,omitempty"`
	AvgFill   int64  `json:"avgFill"` // volume-weighted fill price, ticks
	Ts        int64  `json:"ts"`
	TsEnd     int64  `json:"tsEnd,omitempty"`

	cost int64 // Σ price*qty across fills; AvgFill = cost/Filled
}

// PnL is the netting ledger plus the live mark. All money is tick·lots.
type PnL struct {
	Pos        int64 `json:"pos"` // signed lots; + long, − short
	Avg        int64 `json:"avg"` // average open price, ticks
	Mark       int64 `json:"mark"`
	Realized   int64 `json:"realized"`
	Unrealized int64 `json:"unrealized"`
	Total      int64 `json:"total"`
	Fills      int   `json:"fills"`
	Volume     int64 `json:"volume"` // lots traded
}

type Account struct {
	mu         sync.Mutex
	instrument int
	nextID     uint64

	open   map[uint64]*AccountOrder
	closed []*AccountOrder // newest first

	pos, avg  int64
	realized  int64
	volume    int64
	fills     int
	mark      int64 // mid from the last book snapshot
	lastTrade int64 // fallback mark when the book is one-sided

	structDirty bool // order set changed (tables need a repaint)
	pnlDirty    bool // only the numbers moved
}

func NewAccount(instrument int) *Account {
	return &Account{
		instrument: instrument,
		nextID:     acctIDBase,
		open:       make(map[uint64]*AccountOrder),
	}
}

// NewOrder validates and registers a LIVE order, returning its wire line.
// The order is optimistic: if the engine rejects it, OnReject closes it out.
func (a *Account) NewOrder(side, kind string, price, qty, ts int64) (uint64, string, error) {
	side = strings.ToUpper(side)
	kind = strings.ToUpper(kind)
	switch side {
	case "B", "S":
	default:
		return 0, "", fmt.Errorf("side must be B or S")
	}
	switch kind {
	case "LIMIT", "MARKET", "IOC", "FOK", "POST":
	default:
		return 0, "", fmt.Errorf("kind must be LIMIT, MARKET, IOC, FOK or POST")
	}
	if qty <= 0 {
		return 0, "", fmt.Errorf("qty must be positive")
	}
	if kind != "MARKET" && price <= 0 {
		return 0, "", fmt.Errorf("price must be positive")
	}

	a.mu.Lock()
	defer a.mu.Unlock()
	id := a.nextID
	a.nextID++
	a.open[id] = &AccountOrder{
		ID: id, Side: side, Price: price, Qty: qty,
		Remaining: qty, Kind: kind, Status: stLive, Ts: ts,
	}
	a.structDirty = true
	wire := fmt.Sprintf("NEW %d %d %d %s %d %d %s", a.instrument, id, acctOwner, side, price, qty, kind)
	return id, wire, nil
}

// Cancel returns the wire line for cancelling an open order.
func (a *Account) Cancel(id uint64) (string, error) {
	a.mu.Lock()
	defer a.mu.Unlock()
	if _, ok := a.open[id]; !ok {
		return "", fmt.Errorf("order %d is not open", id)
	}
	return fmt.Sprintf("CANCEL %d %d", a.instrument, id), nil
}

// Reduce returns the wire line for a partial delete: shrinking a resting
// order's remaining size without losing its queue position or its identity.
// Only a strict decrease qualifies — growing an order is cancel/replace (it
// loses queue position on the engine anyway) and isn't "delete" at all, so
// it's rejected here rather than silently reinterpreted.
func (a *Account) Reduce(id uint64, qty int64) (string, error) {
	a.mu.Lock()
	defer a.mu.Unlock()
	o, ok := a.open[id]
	if !ok {
		return "", fmt.Errorf("order %d is not open", id)
	}
	if qty <= 0 {
		return "", fmt.Errorf("qty must be positive; cancel the order instead")
	}
	if qty >= o.Remaining {
		return "", fmt.Errorf("qty must be less than the %d remaining", o.Remaining)
	}
	return fmt.Sprintf("AMEND %d %d %d", a.instrument, id, qty), nil
}

// IsUser reports whether an order id belongs to the web user — used to flag
// the user's own prints in the market-wide trade stream.
func (a *Account) IsUser(ids ...uint64) bool {
	for _, id := range ids {
		if id >= acctIDBase && id < a.nextID {
			return true
		}
	}
	return false
}

// OnFill applies one fill to the order and to the ledger. Returns the order
// for a notification, or nil if the fill was for an unknown id (can happen
// after a reconnect marked everything LOST).
func (a *Account) OnFill(id uint64, price, qty, remaining, ts int64) *AccountOrder {
	a.mu.Lock()
	defer a.mu.Unlock()
	o := a.open[id]
	if o == nil {
		return nil
	}
	o.Filled += qty
	o.cost += price * qty
	o.AvgFill = o.cost / o.Filled
	o.Remaining = remaining

	a.applyFill(o.Side, price, qty)
	a.fills++
	a.volume += qty

	if remaining == 0 {
		a.closeLocked(o, stFilled, "", ts)
	}
	a.pnlDirty = true
	return o
}

// applyFill is the D20 netting ledger: increase re-averages, reduce realizes
// against the average, zero resets, a flip re-opens at the fill price.
func (a *Account) applyFill(side string, price, qty int64) {
	s := qty
	if side == "S" {
		s = -qty
	}
	switch {
	case a.pos == 0 || (a.pos > 0) == (s > 0):
		// Increasing (or opening): volume-weighted re-average.
		absPos := a.pos
		if absPos < 0 {
			absPos = -absPos
		}
		a.avg = (absPos*a.avg + qty*price) / (absPos + qty)
		a.pos += s
	default:
		// Reducing or flipping.
		absPos := a.pos
		if absPos < 0 {
			absPos = -absPos
		}
		closing := qty
		if closing > absPos {
			closing = absPos
		}
		if a.pos > 0 {
			a.realized += closing * (price - a.avg)
		} else {
			a.realized += closing * (a.avg - price)
		}
		a.pos += s
		switch {
		case a.pos == 0:
			a.avg = 0
		case qty > absPos:
			a.avg = price // flipped through zero
		}
	}
}

// OnCancel closes an order cancelled on the engine (by the user, or the
// dropped remainder of a Market/IOC order).
func (a *Account) OnCancel(id uint64, remaining, ts int64) *AccountOrder {
	a.mu.Lock()
	defer a.mu.Unlock()
	o := a.open[id]
	if o == nil {
		return nil
	}
	status := stCancelled
	if o.Filled > 0 {
		status = stPartialCancel
	}
	a.closeLocked(o, status, "", ts)
	return o
}

// OnAmend applies a confirmed quantity change to a resting order. `qty` is
// the new resting size, not the original order size, so `Qty` (the display
// size) shrinks with it — the order now represents less than it did.
func (a *Account) OnAmend(id uint64, qty, ts int64) *AccountOrder {
	a.mu.Lock()
	defer a.mu.Unlock()
	o := a.open[id]
	if o == nil {
		return nil
	}
	o.Remaining = qty
	o.Qty = o.Filled + qty
	a.structDirty = true
	return o
}

// OnReject closes an order the engine refused outright. A reject for an id
// that is not open is a stale cancel reject — the order already closed
// cleanly, so there is nothing to do.
func (a *Account) OnReject(id uint64, reason string, ts int64) *AccountOrder {
	a.mu.Lock()
	defer a.mu.Unlock()
	o := a.open[id]
	if o == nil {
		return nil
	}
	a.closeLocked(o, stRejected, reason, ts)
	return o
}

// OnEngineDown marks every open order LOST: the bridge missed whatever
// happened while it was away, and pretending otherwise would be fiction.
func (a *Account) OnEngineDown(ts int64) {
	a.mu.Lock()
	defer a.mu.Unlock()
	for _, o := range a.open {
		a.closeLocked(o, stLost, "bridge disconnected", ts)
	}
}

func (a *Account) closeLocked(o *AccountOrder, status, reason string, ts int64) {
	o.Status = status
	o.Reason = reason
	o.TsEnd = ts
	delete(a.open, o.ID)
	a.closed = append([]*AccountOrder{o}, a.closed...)
	if len(a.closed) > acctClosedMax {
		a.closed = a.closed[:acctClosedMax]
	}
	a.structDirty = true
}

// OnBook refreshes the mark from the aggregate mid.
func (a *Account) OnBook(b Book) {
	a.mu.Lock()
	defer a.mu.Unlock()
	bb, ba := b.Bids, b.Asks
	switch {
	case len(bb) > 0 && len(ba) > 0:
		a.mark = (bb[0][0] + ba[0][0]) / 2
	case len(bb) > 0:
		a.mark = bb[0][0]
	case len(ba) > 0:
		a.mark = ba[0][0]
	}
	if a.pos != 0 {
		a.pnlDirty = true
	}
}

// OnTrade keeps a last-trade fallback for one-sided books.
func (a *Account) OnTrade(price int64) {
	a.mu.Lock()
	defer a.mu.Unlock()
	a.lastTrade = price
}

// TakeDirty reports and clears the dirty flags.
func (a *Account) TakeDirty() (structDirty, pnlDirty bool) {
	a.mu.Lock()
	defer a.mu.Unlock()
	s, p := a.structDirty, a.pnlDirty
	a.structDirty, a.pnlDirty = false, false
	return s, p
}

// SnapshotOrders copies the open and closed lists, open sorted newest first.
func (a *Account) SnapshotOrders() (open, closed []*AccountOrder) {
	a.mu.Lock()
	defer a.mu.Unlock()
	open = make([]*AccountOrder, 0, len(a.open))
	for _, o := range a.open {
		cp := *o
		open = append(open, &cp)
	}
	sort.Slice(open, func(i, j int) bool { return open[i].Ts > open[j].Ts })
	closed = make([]*AccountOrder, 0, len(a.closed))
	for _, o := range a.closed {
		cp := *o
		closed = append(closed, &cp)
	}
	return open, closed
}

// SnapshotPnl computes the ledger with the current mark.
func (a *Account) SnapshotPnl() PnL {
	a.mu.Lock()
	defer a.mu.Unlock()
	mark := a.mark
	if mark == 0 {
		mark = a.lastTrade
	}
	unreal := int64(0)
	if a.pos != 0 && mark != 0 {
		unreal = a.pos * (mark - a.avg)
	}
	return PnL{
		Pos: a.pos, Avg: a.avg, Mark: mark,
		Realized: a.realized, Unrealized: unreal,
		Total: a.realized + unreal,
		Fills: a.fills, Volume: a.volume,
	}
}
