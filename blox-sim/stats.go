package main

import (
	"fmt"
	"math"
	"sort"
	"time"
)

// Latencies collects round-trip samples and reports percentiles.
//
// Percentiles, not averages. A mean latency hides exactly the thing you care
// about — the tail — and a matching engine's tail is where the allocator, the
// scheduler, and the network show up.
type Latencies struct {
	samples []time.Duration
}

func (l *Latencies) Add(d time.Duration) { l.samples = append(l.samples, d) }

func (l *Latencies) Len() int { return len(l.samples) }

func (l *Latencies) Percentile(p float64) time.Duration {
	if len(l.samples) == 0 {
		return 0
	}
	if !sort.SliceIsSorted(l.samples, func(i, j int) bool { return l.samples[i] < l.samples[j] }) {
		sort.Slice(l.samples, func(i, j int) bool { return l.samples[i] < l.samples[j] })
	}
	// Nearest-rank, clamped.
	idx := int(math.Ceil(p/100*float64(len(l.samples)))) - 1
	if idx < 0 {
		idx = 0
	}
	if idx >= len(l.samples) {
		idx = len(l.samples) - 1
	}
	return l.samples[idx]
}

func (l *Latencies) Mean() time.Duration {
	if len(l.samples) == 0 {
		return 0
	}
	var total time.Duration
	for _, s := range l.samples {
		total += s
	}
	return total / time.Duration(len(l.samples))
}

func (l *Latencies) Report(label string) {
	if l.Len() == 0 {
		fmt.Printf("  %-22s no samples\n", label)
		return
	}
	fmt.Printf("  %-22s n=%d  mean=%v  p50=%v  p90=%v  p99=%v  p99.9=%v  max=%v\n",
		label, l.Len(), round(l.Mean()),
		round(l.Percentile(50)), round(l.Percentile(90)),
		round(l.Percentile(99)), round(l.Percentile(99.9)),
		round(l.Percentile(100)))
}

// round trims sub-100ns noise so output stays readable.
func round(d time.Duration) time.Duration {
	switch {
	case d < time.Microsecond:
		return d.Round(10 * time.Nanosecond)
	case d < time.Millisecond:
		return d.Round(100 * time.Nanosecond)
	default:
		return d.Round(time.Microsecond)
	}
}

func rate(n int64, elapsed time.Duration) float64 {
	if elapsed <= 0 {
		return 0
	}
	return float64(n) / elapsed.Seconds()
}

func humanRate(n int64, elapsed time.Duration) string {
	r := rate(n, elapsed)
	switch {
	case r >= 1e6:
		return fmt.Sprintf("%.2fM/s", r/1e6)
	case r >= 1e3:
		return fmt.Sprintf("%.1fk/s", r/1e3)
	default:
		return fmt.Sprintf("%.0f/s", r)
	}
}
