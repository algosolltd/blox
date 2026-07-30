// Command blox-visualize is a web dashboard for the blox engine: it bridges
// blox-server's TCP protocol to WebSocket and serves a single-page terminal
// with a market chart, trades feed, depth chart and order book.
//
// With no flags it spawns a fresh blox-server and the blox-sim synthetic
// market, so `./blox-visualize` is the whole demo. Use -addr to attach to a
// server you already run, and -no-sim if you'd rather drive it yourself
// (e.g. `nc 127.0.0.1 7070`).
package main

import (
	"bufio"
	"context"
	"embed"
	"errors"
	"flag"
	"fmt"
	"io/fs"
	"net/http"
	"os"
	"os/exec"
	"os/signal"
	"strconv"
	"strings"
	"syscall"
	"time"
)

//go:embed static
var staticFS embed.FS

func main() {
	var (
		listen     = flag.String("listen", "127.0.0.1:8080", "web UI listen address")
		addr       = flag.String("addr", "", "blox-server address (default: spawn one)")
		serverBin  = flag.String("server", "../target/release/blox-server", "server binary to spawn when -addr is empty")
		simBin     = flag.String("sim", "../blox-sim/blox-sim", "blox-sim binary to spawn")
		noSim      = flag.Bool("no-sim", false, "do not spawn the market simulator")
		duration   = flag.Duration("duration", 8*time.Hour, "simulator run time")
		seed       = flag.Uint64("seed", 42, "simulator PRNG seed")
		makers     = flag.Int("makers", 8, "simulator market makers (more = deeper book)")
		noise      = flag.Int("noise", 4, "simulator noise traders (fewer = quotes survive longer)")
		momentum   = flag.Int("momentum", 2, "simulator momentum traders")
		instrument = flag.Int("instrument", 1, "instrument id to visualize")
		depth      = flag.Int("depth", 40, "aggregate book depth to poll")
		bookEvery  = flag.Duration("book-every", 100*time.Millisecond, "book poll interval")
		name       = flag.String("name", "BLOX/USD", "display name of the instrument")
		priceScale = flag.Int("price-scale", 2, "display decimals for prices")
		staticDir  = flag.String("static-dir", "", "serve the UI from this directory instead of the embedded copy (development)")
	)
	flag.Parse()

	ctx, stop := signal.NotifyContext(context.Background(), os.Interrupt, syscall.SIGTERM)
	defer stop()

	target := *addr
	var kids []*exec.Cmd
	defer func() {
		for _, k := range kids {
			_ = k.Process.Kill()
			_ = k.Wait()
		}
	}()

	if target == "" {
		a, cmd, err := spawnServer(*serverBin)
		if err != nil {
			fmt.Fprintf(os.Stderr, "blox-visualize: %v\n", err)
			os.Exit(1)
		}
		target = a
		kids = append(kids, cmd)
		fmt.Printf("blox-server listening on %s\n", target)
	}

	if !*noSim {
		cmd, err := spawnSim(*simBin, target, *duration, *seed, *makers, *noise, *momentum)
		if err != nil {
			fmt.Fprintf(os.Stderr, "blox-visualize: simulator not started: %v\n", err)
			fmt.Fprintf(os.Stderr, "  (the UI still works — drive %s yourself, e.g. nc %s)\n", target, strings.Replace(target, ":", " ", 1))
		} else {
			kids = append(kids, cmd)
			fmt.Printf("blox-sim market on %s (seed %d, %d makers, %d noise, %d momentum, %s)\n",
				target, *seed, *makers, *noise, *momentum, *duration)
		}
	}

	events := make(chan any, 4096)
	engineIn := make(chan string, 256)
	eng := &Engine{
		Addr:       target,
		Instrument: *instrument,
		Depth:      *depth,
		BookEvery:  *bookEvery,
		StatsEvery: time.Second,
		Out:        events,
		In:         engineIn,
	}
	go eng.Run(ctx)

	hub := NewHub(*name, *instrument, *priceScale, target, NewAccount(*instrument), engineIn)
	go hub.Run(events)

	mux := http.NewServeMux()
	mux.HandleFunc("/ws", hub.ServeWS)
	if *staticDir != "" {
		mux.Handle("/", http.FileServer(http.Dir(*staticDir)))
	} else {
		sub, err := fs.Sub(staticFS, "static")
		if err != nil {
			fmt.Fprintf(os.Stderr, "blox-visualize: %v\n", err)
			os.Exit(1)
		}
		mux.Handle("/", http.FileServer(http.FS(sub)))
	}

	srv := &http.Server{Addr: *listen, Handler: mux, ReadHeaderTimeout: 5 * time.Second}
	go func() {
		<-ctx.Done()
		shutCtx, cancel := context.WithTimeout(context.Background(), 2*time.Second)
		defer cancel()
		_ = srv.Shutdown(shutCtx)
	}()

	fmt.Printf("blox-visualize → http://%s\n", *listen)
	if err := srv.ListenAndServe(); err != nil && !errors.Is(err, http.ErrServerClosed) {
		fmt.Fprintf(os.Stderr, "blox-visualize: %v\n", err)
		os.Exit(1)
	}
}

// spawnServer starts blox-server on an ephemeral port and returns the
// address it reports on stdout ("listening <addr>").
func spawnServer(bin string) (string, *exec.Cmd, error) {
	if _, err := os.Stat(bin); err != nil {
		return "", nil, fmt.Errorf("server binary %s: %w (build it first: cargo build --release)", bin, err)
	}
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
		}
		return r.addr, cmd, r.err
	case <-time.After(5 * time.Second):
		_ = cmd.Process.Kill()
		return "", nil, fmt.Errorf("server did not start within 5s")
	}
}

// spawnSim starts the synthetic market against an already-running server.
func spawnSim(bin, addr string, d time.Duration, seed uint64, makers, noise, momentum int) (*exec.Cmd, error) {
	if _, err := os.Stat(bin); err != nil {
		return nil, fmt.Errorf("sim binary %s: %w (build it: cd blox-sim && go build -o blox-sim .)", bin, err)
	}
	cmd := exec.Command(bin,
		"-addr", addr,
		"-mode", "market",
		"-duration", d.String(),
		"-seed", strconv.FormatUint(seed, 10),
		"-makers", strconv.Itoa(makers),
		"-noise", strconv.Itoa(noise),
		"-momentum", strconv.Itoa(momentum),
	)
	cmd.Stdout = os.Stdout
	cmd.Stderr = os.Stderr
	if err := cmd.Start(); err != nil {
		return nil, err
	}
	return cmd, nil
}
