package main

import (
	"fmt"
	"strconv"
	"strings"
	"sync"
	"sync/atomic"
	"time"
)

// A fake market: one liquidity provider publishing a reference price, market
// makers quoting around it, and takers hitting those quotes.
//
// The three archetypes are what make a book feel alive. A random-walk price on
// its own feels dead because nothing reacts to anything: spreads never widen
// under pressure, depth never gets consumed and rebuilt.
//
//	market maker  quotes both sides, re-quotes when the reference moves
//	noise trader  random aggressive orders — refills, temporary impact
//	momentum      buys strength, sells weakness — trends and their ends

const instrument = 1

// Order-id namespaces. Every client gets a disjoint 1e9-wide range.
//
// Modes share one engine, so overlapping ranges make the engine reject
// duplicate ids — correct behaviour that presents as a mysterious reject
// count. Keep these far apart and each range far larger than any run needs.
const (
	idSpan           = 1_000_000_000
	idBaseAgents     = 1 * idSpan // + agent index
	idBaseObserver   = 100 * idSpan
	idBaseLatency    = 200 * idSpan
	idBaseProbe      = 250 * idSpan
	idBaseThroughput = 300 * idSpan // + connection index
)

// Market is the shared state and counters for a simulation run.
type Market struct {
	// Reference price in ticks, owned by the LP goroutine. Market makers read
	// it directly rather than inferring it from the aggregate: an MM has its
	// own fair-value model, and reading back its own quotes would feed back.
	refPrice atomic.Int64

	trades  atomic.Int64
	volume  atomic.Int64
	orders  atomic.Int64
	rejects atomic.Int64
	// Cancels of orders that had already filled. Expected and benign: a
	// market maker's quote can be lifted between deciding to cancel and the
	// cancel arriving. Counted apart so a non-zero `rejects` stays meaningful.
	staleCancels atomic.Int64
	cancels      atomic.Int64
	snapshots    atomic.Int64

	stop chan struct{}
	wg   sync.WaitGroup
}

func NewMarket(initialPrice int64) *Market {
	m := &Market{stop: make(chan struct{})}
	m.refPrice.Store(initialPrice)
	return m
}

func (m *Market) Stop() {
	close(m.stop)
	m.wg.Wait()
}

func (m *Market) done() bool {
	select {
	case <-m.stop:
		return true
	default:
		return false
	}
}

// rng is a seeded xorshift. Deterministic per agent so a failing run can be
// reproduced with the same -seed.
type rng struct{ s uint64 }

func newRNG(seed uint64) *rng { return &rng{s: seed | 1} }

func (r *rng) next() uint64 {
	r.s ^= r.s >> 12
	r.s ^= r.s << 25
	r.s ^= r.s >> 27
	return r.s * 0x2545F4914F6CDD1D
}

func (r *rng) below(n int64) int64 {
	if n <= 0 {
		return 0
	}
	return int64(r.next() % uint64(n))
}

// span returns a value in [-n, n].
func (r *rng) span(n int64) int64 { return r.below(2*n+1) - n }

// LiquidityProvider random-walks a reference price and publishes it as a
// provider snapshot, exercising the LevelBook path.
func (m *Market) LiquidityProvider(addr string, seed uint64, interval time.Duration, depth int) error {
	// The LP only publishes snapshots, which carry no order ids.
	c, err := Dial(addr, 0)
	if err != nil {
		return err
	}
	m.wg.Add(1)
	go func() {
		defer m.wg.Done()
		defer c.Close()
		// Quiet: the LP does not need acks, and they would double its traffic.
		_ = c.Quiet(true)
		r := newRNG(seed)
		tick := time.NewTicker(interval)
		defer tick.Stop()

		for !m.done() {
			select {
			case <-m.stop:
				return
			case <-tick.C:
			}

			ref := m.refPrice.Load() + r.span(3)
			if ref < 1000 {
				ref = 1000 // keep prices sane; a walk to zero proves nothing
			}
			m.refPrice.Store(ref)

			var bids, asks []string
			for d := 0; d < depth; d++ {
				dd := int64(d)
				bids = append(bids, strconv.FormatInt(ref-2-dd*2, 10)+":"+strconv.FormatInt(10+r.below(40), 10))
				asks = append(asks, strconv.FormatInt(ref+2+dd*2, 10)+":"+strconv.FormatInt(10+r.below(40), 10))
			}
			if err := c.Send("SNAP 10 %d %s %s", instrument, strings.Join(bids, ","), strings.Join(asks, ",")); err != nil {
				return
			}
			if err := c.Flush(); err != nil {
				return
			}
			m.snapshots.Add(1)
		}
	}()
	return nil
}

// MarketMaker keeps a two-sided quote in the internal book and re-quotes when
// the reference moves away from it.
func (m *Market) MarketMaker(addr string, idx int, seed uint64, spread int64, size int64, interval time.Duration) error {
	c, err := Dial(addr, idBaseAgents+uint64(idx)*idSpan)
	if err != nil {
		return err
	}
	m.wg.Add(1)
	go func() {
		defer m.wg.Done()
		defer c.Close()
		_ = c.Quiet(true)
		go m.drainTop(c, nil)

		r := newRNG(seed)
		owner := 100 + idx
		var bidID, askID uint64
		var quotedAt int64
		tick := time.NewTicker(interval)
		defer tick.Stop()

		for !m.done() {
			select {
			case <-m.stop:
				return
			case <-tick.C:
			}

			ref := m.refPrice.Load()
			// Re-quote only when the market has actually moved. Cancelling and
			// replacing on every tick would measure cancel throughput rather
			// than anything about market making.
			if bidID != 0 && abs(ref-quotedAt) < spread/2 {
				continue
			}

			if bidID != 0 {
				_ = c.Send("CANCEL %d %d", instrument, bidID)
				_ = c.Send("CANCEL %d %d", instrument, askID)
				m.cancels.Add(2)
			}

			jitter := r.span(2)
			bidID, askID = c.ID(), c.ID()
			qty := size + r.below(size)
			_ = c.Send("NEW %d %d %d B %d %d LIMIT", instrument, bidID, owner, ref-spread/2+jitter, qty)
			_ = c.Send("NEW %d %d %d S %d %d LIMIT", instrument, askID, owner, ref+spread/2+jitter, qty)
			m.orders.Add(2)
			quotedAt = ref

			if c.Flush() != nil {
				return
			}
		}
	}()
	return nil
}

// NoiseTrader sends aggressive orders in random directions. It is what makes
// depth get consumed and rebuilt.
func (m *Market) NoiseTrader(addr string, idx int, seed uint64, size int64, interval time.Duration) error {
	c, err := Dial(addr, idBaseAgents+uint64(idx)*idSpan)
	if err != nil {
		return err
	}
	m.wg.Add(1)
	go func() {
		defer m.wg.Done()
		defer c.Close()
		_ = c.Quiet(true)
		go m.drainTop(c, nil)

		r := newRNG(seed)
		owner := 200 + idx
		tick := time.NewTicker(interval)
		defer tick.Stop()

		for !m.done() {
			select {
			case <-m.stop:
				return
			case <-tick.C:
			}

			ref := m.refPrice.Load()
			side, price := "B", ref+20 // cross well through the quotes
			if r.below(2) == 0 {
				side, price = "S", ref-20
			}
			// IOC: take what is there, never leave a resting remainder. A
			// noise trader that accumulated resting orders would slowly turn
			// into a market maker and stop being noise.
			_ = c.Send("NEW %d %d %d %s %d %d IOC", instrument, c.ID(), owner, side, price, 1+r.below(size))
			m.orders.Add(1)
			if c.Flush() != nil {
				return
			}
		}
	}()
	return nil
}

// MomentumTrader buys strength and sells weakness off its own view of the top
// of book, which it maintains from the subscription stream.
func (m *Market) MomentumTrader(addr string, idx int, seed uint64, lookback int, size int64, interval time.Duration) error {
	c, err := Dial(addr, idBaseAgents+uint64(idx)*idSpan)
	if err != nil {
		return err
	}
	// Quiet before Sub, so no pushed event can be read in place of an ack.
	if err := c.Quiet(true); err != nil {
		c.Close()
		return err
	}
	if resp, err := c.Do("SUB %d", instrument); err != nil || resp != "OK" {
		c.Close()
		return err
	}
	m.wg.Add(1)
	go func() {
		defer m.wg.Done()
		defer c.Close()

		var mu sync.Mutex
		var mids []int64
		go m.drainTop(c, func(t Top) {
			mu.Lock()
			mids = append(mids, t.Mid())
			if len(mids) > lookback {
				mids = mids[len(mids)-lookback:]
			}
			mu.Unlock()
		})

		r := newRNG(seed)
		owner := 300 + idx
		tick := time.NewTicker(interval)
		defer tick.Stop()

		for !m.done() {
			select {
			case <-m.stop:
				return
			case <-tick.C:
			}

			mu.Lock()
			n := len(mids)
			var first, last int64
			if n >= 2 {
				first, last = mids[0], mids[n-1]
			}
			mu.Unlock()
			if n < 2 || first == last {
				continue
			}

			ref := m.refPrice.Load()
			side, price := "B", ref+20
			if last < first {
				side, price = "S", ref-20
			}
			_ = c.Send("NEW %d %d %d %s %d %d IOC", instrument, c.ID(), owner, side, price, 1+r.below(size))
			m.orders.Add(1)
			if c.Flush() != nil {
				return
			}
		}
	}()
	return nil
}

// Observer is the single subscriber that counts market-wide activity.
//
// Exactly one, deliberately. Trade events go to every subscriber, so counting
// them inside each agent would multiply the total by the number of subscribed
// agents — a bug that looks like a suspiciously fast engine.
type Observer struct {
	c       *Client
	settled chan struct{}
}

func (m *Market) StartObserver(addr string) (*Observer, error) {
	c, err := Dial(addr, idBaseObserver)
	if err != nil {
		return nil, err
	}
	// Quiet before Sub: once subscribed, pushed events would race the ack of
	// any later command and be read in its place.
	if err := c.Quiet(true); err != nil {
		c.Close()
		return nil, err
	}
	if resp, err := c.Do("SUB %d", instrument); err != nil || resp != "OK" {
		c.Close()
		if err == nil {
			err = fmt.Errorf("SUB: %s", resp)
		}
		return nil, err
	}

	o := &Observer{c: c, settled: make(chan struct{})}
	go func() {
		for {
			line, err := c.ReadLine()
			if err != nil {
				close(o.settled)
				return
			}
			// Our own barrier. Reaching this means every event queued to us
			// before the ping has been delivered and counted.
			if line == "PONG settle" {
				close(o.settled)
				return
			}
			f := strings.Fields(line)
			if len(f) < 2 || f[0] != "EV" || f[1] != "TRADE" {
				continue
			}
			m.trades.Add(1)
			if len(f) >= 5 {
				if q, err := strconv.ParseInt(f[4], 10, 64); err == nil {
					m.volume.Add(q)
				}
			}
		}
	}()
	return o, nil
}

// Settle waits until every event already queued to the observer has been
// counted, then closes it.
//
// Pinging on a *different* connection would only prove the engine drained its
// input, not that this subscriber received the resulting pushes.
func (o *Observer) Settle() {
	_ = o.c.Send("PING settle")
	_ = o.c.Flush()
	select {
	case <-o.settled:
	case <-time.After(10 * time.Second):
	}
	o.c.Close()
}

// drainTop consumes a connection's inbound stream, tracking that agent's own
// rejects and optionally handing it parsed tops.
//
// Every agent must drain something. A client that stops reading fills its
// server-side queue and gets disconnected by design (DESIGN.md D25) — correct
// behaviour, and exactly what this avoids triggering.
func (m *Market) drainTop(c *Client, onTop func(Top)) {
	for {
		line, err := c.ReadLine()
		if err != nil {
			return
		}
		f := strings.Fields(line)
		if len(f) < 2 || f[0] != "EV" {
			continue
		}
		switch f[1] {
		case "REJECT":
			if len(f) >= 4 && f[3] == "UnknownOrder" {
				m.staleCancels.Add(1)
			} else {
				m.rejects.Add(1)
			}
		case "TOP":
			if onTop != nil {
				if t, ok := parseTop(f[1:]); ok {
					onTop(t)
				}
			}
		}
	}
}

func abs(v int64) int64 {
	if v < 0 {
		return -v
	}
	return v
}
