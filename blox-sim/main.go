// Command blox-sim drives blox-server with a synthetic market and measures it.
//
// Three modes, measuring different things:
//
//	market      a realistic multi-agent market — makers, noise, momentum.
//	            Checks the engine stays correct under concurrent load.
//	throughput  pipelined events, to find the ceiling in events/sec.
//	latency     one command at a time, for round-trip percentiles.
//
// Everything measured here is the *whole* path: Go, socket, protocol, engine.
// Rust-side micro-benchmarks (`cargo bench`) measure the engine alone. The gap
// between the two numbers is the point — see docs/BENCHMARKS.md.
package main

import (
	"bufio"
	"flag"
	"fmt"
	"os"
	"os/exec"
	"strings"
	"sync"
	"time"
)

func main() {
	var (
		addr     = flag.String("addr", "", "blox-server address (default: spawn one)")
		mode     = flag.String("mode", "all", "market | throughput | latency | all")
		duration = flag.Duration("duration", 5*time.Second, "market mode run time")
		seed     = flag.Uint64("seed", 42, "PRNG seed — same seed, same run")
		makers   = flag.Int("makers", 4, "market makers")
		noise    = flag.Int("noise", 6, "noise traders")
		momentum = flag.Int("momentum", 2, "momentum traders")
		conns    = flag.Int("conns", 4, "throughput mode connections")
		events   = flag.Int("events", 200_000, "throughput mode events per connection")
		samples  = flag.Int("samples", 20_000, "latency mode samples")
		server   = flag.String("server", "../target/release/blox-server", "server binary to spawn")
	)
	flag.Parse()

	target := *addr
	var proc *exec.Cmd
	if target == "" {
		var err error
		target, proc, err = spawnServer(*server)
		if err != nil {
			fmt.Fprintf(os.Stderr, "could not start server: %v\n", err)
			fmt.Fprintf(os.Stderr, "build it first: cargo build --release\n")
			os.Exit(1)
		}
		defer func() {
			_ = proc.Process.Kill()
			_ = proc.Wait()
		}()
	}
	fmt.Printf("blox-sim -> %s\n\n", target)

	ok := true
	switch *mode {
	case "market":
		ok = runMarket(target, *duration, *seed, *makers, *noise, *momentum)
	case "throughput":
		ok = runThroughput(target, *conns, *events)
	case "latency":
		ok = runLatency(target, *samples)
	case "all":
		// Order matters: all three modes share one engine, so each sees what
		// the previous one left behind. Market goes first because it is the
		// only one whose numbers depend on starting from a clean book;
		// throughput goes last because it leaves ~800k resting orders.
		ok = runMarket(target, *duration, *seed, *makers, *noise, *momentum)
		fmt.Println()
		ok = runLatency(target, *samples) && ok
		fmt.Println()
		ok = runThroughput(target, *conns, *events) && ok
	default:
		fmt.Fprintf(os.Stderr, "unknown mode %q\n", *mode)
		os.Exit(2)
	}

	if !ok {
		os.Exit(1)
	}
}

// spawnServer starts the server on an ephemeral port and waits for it to
// report the address it actually bound.
func spawnServer(bin string) (string, *exec.Cmd, error) {
	cmd := exec.Command(bin, "127.0.0.1:0")
	stdout, err := cmd.StdoutPipe()
	if err != nil {
		return "", nil, err
	}
	cmd.Stderr = os.Stderr
	if err := cmd.Start(); err != nil {
		return "", nil, err
	}

	type result struct {
		addr string
		err  error
	}
	ch := make(chan result, 1)
	go func() {
		sc := bufio.NewScanner(stdout)
		for sc.Scan() {
			if a, ok := strings.CutPrefix(sc.Text(), "listening "); ok {
				ch <- result{addr: a}
				return
			}
		}
		ch <- result{err: fmt.Errorf("server exited without reporting an address")}
	}()

	select {
	case r := <-ch:
		if r.err != nil {
			_ = cmd.Process.Kill()
			return "", nil, r.err
		}
		return r.addr, cmd, nil
	case <-time.After(5 * time.Second):
		_ = cmd.Process.Kill()
		return "", nil, fmt.Errorf("server did not start within 5s")
	}
}

// ---- market -------------------------------------------------------------

func runMarket(addr string, d time.Duration, seed uint64, makers, noise, momentum int) bool {
	fmt.Printf("== market: %d makers, %d noise, %d momentum, %v ==\n",
		makers, noise, momentum, d)

	m := NewMarket(10_000)
	idx := 1

	// One observer counts trades for the whole market.
	obsv, err := m.StartObserver(addr)
	if err != nil {
		fmt.Fprintf(os.Stderr, "observer: %v\n", err)
		return false
	}

	if err := m.LiquidityProvider(addr, seed, time.Millisecond, quoteDepth); err != nil {
		fmt.Fprintf(os.Stderr, "LP: %v\n", err)
		return false
	}
	for i := 0; i < makers; i++ {
		if err := m.MarketMaker(addr, idx, seed+uint64(idx), 20, quoteDepth, time.Millisecond); err != nil {
			fmt.Fprintf(os.Stderr, "maker %d: %v\n", i, err)
			return false
		}
		idx++
	}
	for i := 0; i < noise; i++ {
		if err := m.NoiseTrader(addr, idx, seed+uint64(idx), 15, 400*time.Microsecond); err != nil {
			fmt.Fprintf(os.Stderr, "noise %d: %v\n", i, err)
			return false
		}
		idx++
	}
	for i := 0; i < momentum; i++ {
		if err := m.MomentumTrader(addr, idx, seed+uint64(idx), 8, 10, 1*time.Millisecond); err != nil {
			fmt.Fprintf(os.Stderr, "momentum %d: %v\n", i, err)
			return false
		}
		idx++
	}

	start := time.Now()
	time.Sleep(d)
	m.Stop()
	elapsed := time.Since(start)

	// Drain: first the engine's own input, then the observer's inbound queue.
	obs, err := Dial(addr, idBaseProbe)
	if err != nil {
		fmt.Fprintf(os.Stderr, "probe: %v\n", err)
		return false
	}
	defer obs.Close()
	if err := obs.Ping("settle"); err != nil {
		fmt.Fprintf(os.Stderr, "settle: %v\n", err)
		return false
	}
	obsv.Settle()

	orders := m.orders.Load()
	trades := m.trades.Load()
	fmt.Printf("  orders submitted     %d (%s)\n", orders, humanRate(orders, elapsed))
	fmt.Printf("  cancels              %d\n", m.cancels.Load())
	fmt.Printf("  provider snapshots   %d (%s)\n", m.snapshots.Load(), humanRate(m.snapshots.Load(), elapsed))
	fmt.Printf("  trades matched       %d (%s)\n", trades, humanRate(trades, elapsed))
	fmt.Printf("  volume               %d lots\n", m.volume.Load())
	fmt.Printf("  stale cancels        %d (quote filled before the cancel landed)\n", m.staleCancels.Load())
	fmt.Printf("  rejects              %d\n", m.rejects.Load())

	top, err := obs.Do("TOP %d", instrument)
	if err == nil {
		fmt.Printf("  final top of book    %s\n", top)
	}
	if stats, err := obs.Do("STATS"); err == nil {
		fmt.Printf("  engine               %s\n", strings.TrimPrefix(stats, "STATS "))
	}

	// The point of the whole exercise: is the book still correct after being
	// hammered by concurrent agents?
	check, err := obs.Do("CHECK")
	if err != nil {
		fmt.Fprintf(os.Stderr, "CHECK failed: %v\n", err)
		return false
	}
	if !strings.HasPrefix(check, "OK") {
		fmt.Printf("  INVARIANT BREACH     %s\n", check)
		return false
	}
	fmt.Printf("  invariants           %s\n", check)

	if trades == 0 {
		fmt.Println("  WARNING: no trades matched — the run proved nothing")
		return false
	}
	return true
}

// ---- throughput ---------------------------------------------------------

func runThroughput(addr string, conns, perConn int) bool {
	fmt.Printf("== throughput: %d connections x %d events ==\n", conns, perConn)

	clients := make([]*Client, conns)
	for i := range clients {
		c, err := Dial(addr, idBaseThroughput+uint64(i)*idSpan)
		if err != nil {
			fmt.Fprintf(os.Stderr, "dial: %v\n", err)
			return false
		}
		defer c.Close()
		// No acks, no lifecycle echo, no subscription: measure the engine
		// rather than the reply path, and never fill our own inbound queue.
		if err := c.Quiet(true); err != nil {
			fmt.Fprintf(os.Stderr, "quiet: %v\n", err)
			return false
		}
		if err := c.Echo(false); err != nil {
			fmt.Fprintf(os.Stderr, "echo: %v\n", err)
			return false
		}
		clients[i] = c
	}

	// Warm up so the measured run does not include first-touch page faults
	// and the server's initial allocations.
	for i, c := range clients {
		owner := 400 + i
		for n := 0; n < 2_000; n++ {
			_ = c.Send("NEW %d %d %d B %d 5 LIMIT", instrument, c.ID(), owner, 9_000+int64(n%50))
		}
		_ = c.Flush()
	}
	for _, c := range clients {
		_ = c.Ping("warm")
	}

	var wg sync.WaitGroup
	start := time.Now()
	for i, c := range clients {
		wg.Add(1)
		go func(i int, c *Client) {
			defer wg.Done()
			r := newRNG(uint64(i)*7919 + 13)
			owner := 500 + i
			for n := 0; n < perConn; n++ {
				// A realistic mix: mostly resting limit orders with some
				// aggressive IOC. All in a price band tight enough that
				// matching actually happens.
				if n%4 == 3 {
					side, px := "B", int64(10_030)
					if r.below(2) == 0 {
						side, px = "S", 9_970
					}
					_ = c.Send("NEW %d %d %d %s %d %d IOC",
						instrument, c.ID(), owner, side, px, 1+r.below(10))
				} else {
					side, px := "B", 9_990-r.below(20)
					if r.below(2) == 0 {
						side, px = "S", 10_010+r.below(20)
					}
					_ = c.Send("NEW %d %d %d %s %d %d LIMIT",
						instrument, c.ID(), owner, side, px, 1+r.below(20))
				}
			}
			if err := c.Flush(); err != nil {
				return
			}
			// Barrier: return only once the engine has consumed all of it.
			_ = c.Ping(fmt.Sprintf("done%d", i))
		}(i, c)
	}
	wg.Wait()
	elapsed := time.Since(start)

	total := int64(conns * perConn)
	fmt.Printf("  events               %d in %v\n", total, elapsed.Round(time.Millisecond))
	fmt.Printf("  rate                 %s\n", humanRate(total, elapsed))
	fmt.Printf("  per-event            %v\n", round(elapsed/time.Duration(total)))

	if stats, err := clients[0].Do("STATS"); err == nil {
		fmt.Printf("  engine               %s\n", strings.TrimPrefix(stats, "STATS "))
	}
	if check, err := clients[0].Do("CHECK"); err == nil && !strings.HasPrefix(check, "OK") {
		fmt.Printf("  INVARIANT BREACH     %s\n", check)
		return false
	}
	return true
}

// ---- latency ------------------------------------------------------------

func runLatency(addr string, samples int) bool {
	fmt.Printf("== latency: %d sequential round trips ==\n", samples)

	c, err := Dial(addr, idBaseLatency)
	if err != nil {
		fmt.Fprintf(os.Stderr, "dial: %v\n", err)
		return false
	}
	defer c.Close()
	// Acks on: each command produces exactly one response to wait for.
	if err := c.Quiet(false); err != nil {
		return false
	}

	for n := 0; n < 1_000; n++ {
		if _, err := c.Do("NEW %d %d 600 B %d 5 LIMIT", instrument, c.ID(), 9_000+int64(n%50)); err != nil {
			return false
		}
	}

	var order, ping, query Latencies
	r := newRNG(0xBEEF)

	for n := 0; n < samples; n++ {
		side, px := "B", 9_990-r.below(20)
		if r.below(2) == 0 {
			side, px = "S", 10_010+r.below(20)
		}
		t0 := time.Now()
		if _, err := c.Do("NEW %d %d 600 %s %d %d LIMIT", instrument, c.ID(), side, px, 1+r.below(10)); err != nil {
			return false
		}
		order.Add(time.Since(t0))
	}

	for n := 0; n < samples/4; n++ {
		t0 := time.Now()
		if _, err := c.Do("PING x"); err != nil {
			return false
		}
		ping.Add(time.Since(t0))

		t0 = time.Now()
		if _, err := c.Do("TOP %d", instrument); err != nil {
			return false
		}
		query.Add(time.Since(t0))
	}

	// PING does nothing but traverse the path, so it is the floor: the socket
	// and protocol cost with no engine work. The gap to `new order` is what
	// the matching actually costs at this level.
	ping.Report("ping (path floor)")
	query.Report("top-of-book query")
	order.Report("new order")

	fmt.Printf("  engine work per order ~%v (new order p50 - ping p50)\n",
		round(order.Percentile(50)-ping.Percentile(50)))
	return true
}
