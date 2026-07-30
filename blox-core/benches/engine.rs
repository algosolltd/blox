//! Engine micro-benchmarks, against the targets in `docs/CORE.md` §11.
//!
//! No criterion, no dependency: `harness = false` plus `Instant` is enough to
//! answer "is this in the right order of magnitude", which is the only
//! question that matters here. The engine is ~4 orders of magnitude faster
//! than the network hop in front of it (`docs/BENCHMARKS.md`), so precision
//! beyond that would be measuring something nobody can spend.
//!
//! Run with: `cargo bench -p blox-core`

use std::hint::black_box;
use std::time::Instant;

use blox_core::testkit::scripted_events;
use blox_core::*;

const I: InstrumentId = InstrumentId(1);

struct Bench {
    name: &'static str,
    target_ns: f64,
    ns: f64,
}

fn main() {
    println!("blox-core micro-benchmarks (single-threaded, no I/O)\n");
    let mut results = vec![
        bench_delta_existing_level(),
        bench_delta_new_level(),
        bench_new_limit_no_cross(),
        bench_new_crossing_three_levels(),
        bench_cancel(),
        bench_aggregate_five_providers(),
        bench_top_of_book(),
        bench_trigger_check_idle(),
        bench_mixed_workload(),
    ];
    results.sort_by(|a, b| a.name.cmp(b.name));

    println!(
        "{:<38} {:>10} {:>10} {:>9}",
        "operation", "ns/op", "target", "ops/sec"
    );
    println!("{}", "-".repeat(70));
    for r in &results {
        let target = if r.target_ns > 0.0 {
            format!("{:.0}", r.target_ns)
        } else {
            "-".to_string()
        };
        println!(
            "{:<38} {:>10.1} {:>10} {:>9}",
            r.name,
            r.ns,
            target,
            human(1e9 / r.ns)
        );
    }

    let order_ns = results
        .iter()
        .find(|r| r.name.contains("new limit"))
        .map_or(50.0, |r| r.ns);
    println!(
        "\nOne network round trip to a broker is 20-40ms — about {}x the cost\n\
         of matching an order. That ratio is why DESIGN.md D6 stops here.",
        human(30e6 / order_ns)
    );
}

fn human(v: f64) -> String {
    match v {
        v if v >= 1e9 => format!("{:.1}G", v / 1e9),
        v if v >= 1e6 => format!("{:.1}M", v / 1e6),
        v if v >= 1e3 => format!("{:.1}k", v / 1e3),
        v => format!("{v:.0}"),
    }
}

/// Time `iters` calls of `op` after an unmeasured warm-up, and take the best
/// of several rounds. The minimum is the most stable estimate when the noise
/// is additive — scheduler, interrupts, other processes.
///
/// Only use this when `op` performs exactly **one** engine operation.
/// Anything else reports the cost of the pair under the name of one of them.
fn measure(name: &'static str, target_ns: f64, iters: usize, mut op: impl FnMut(usize)) -> Bench {
    for i in 0..iters.min(10_000) {
        op(i);
    }
    let mut best = f64::MAX;
    for _ in 0..5 {
        let t = Instant::now();
        for i in 0..iters {
            op(i);
        }
        let ns = t.elapsed().as_nanos() as f64 / iters as f64;
        best = best.min(ns);
    }
    Bench {
        name,
        target_ns,
        ns: best,
    }
}

/// For operations that need per-round setup that must not be timed — e.g.
/// cancelling requires orders to exist first, and timing the setup alongside
/// would report the cost of "rest + cancel" under the name "cancel".
///
/// `round` does its own setup, then returns the timed span and how many
/// measured operations it covered.
fn best_of(
    name: &'static str,
    target_ns: f64,
    rounds: usize,
    mut round: impl FnMut() -> (std::time::Duration, usize),
) -> Bench {
    let _ = round(); // warm up, discarded
    let mut best = f64::MAX;
    for _ in 0..rounds {
        let (elapsed, n) = round();
        if n > 0 {
            best = best.min(elapsed.as_nanos() as f64 / n as f64);
        }
    }
    Bench {
        name,
        target_ns,
        ns: best,
    }
}

fn snapshot(e: &mut Engine, p: u16, mid: Ticks, depth: usize) {
    let bids: Vec<_> = (0..depth as i64).map(|d| (mid - 1 - d, 10 + d)).collect();
    let asks: Vec<_> = (0..depth as i64).map(|d| (mid + 1 + d, 10 + d)).collect();
    e.apply(&Event::Snapshot {
        provider: ProviderId(p),
        instrument: I,
        bids,
        asks,
    });
}

fn bench_delta_existing_level() -> Bench {
    let mut e = Engine::new();
    snapshot(&mut e, 10, 10_000, 20);
    let mut out = Vec::with_capacity(64);
    measure("delta, existing level", 20.0, 2_000_000, move |i| {
        out.clear();
        // Deep in the book, so it never moves the top: the common case.
        e.apply_into(
            &Event::Delta {
                provider: ProviderId(10),
                instrument: I,
                side: Side::Bid,
                price: 9_990,
                qty: 1 + (i as Lots % 97),
            },
            &mut out,
        );
        black_box(&out);
    })
}

fn bench_delta_new_level() -> Bench {
    let mut e = Engine::new();
    snapshot(&mut e, 10, 10_000, 20);
    let mut out = Vec::with_capacity(64);
    measure("delta, insert + remove level", 0.0, 1_000_000, move |i| {
        out.clear();
        let price = 9_950 - (i as Ticks % 20);
        for qty in [5, 0] {
            e.apply_into(
                &Event::Delta {
                    provider: ProviderId(10),
                    instrument: I,
                    side: Side::Bid,
                    price,
                    qty,
                },
                &mut out,
            );
        }
        black_box(&out);
    })
}

fn bench_new_limit_no_cross() -> Bench {
    const N: usize = 200_000;
    best_of("new limit order, no cross", 50.0, 5, || {
        // Untimed: a fresh engine, so the arena grows during the measurement
        // exactly as it would when a book is filling up.
        let mut e = Engine::new();
        let mut out = Vec::with_capacity(64);
        let t = Instant::now();
        for i in 0..N {
            out.clear();
            e.apply_into(
                &Event::New {
                    instrument: I,
                    id: i as OrderId + 1,
                    owner: 1,
                    side: Side::Bid,
                    price: 9_900 + (i as Ticks % 50),
                    qty: 10,
                    kind: OrderKind::Limit,
                },
                &mut out,
            );
            black_box(&out);
        }
        let elapsed = t.elapsed();
        black_box(&e);
        (elapsed, N)
    })
}

fn bench_new_crossing_three_levels() -> Bench {
    let mut e = Engine::new();
    let mut out = Vec::with_capacity(256);
    let mut id: OrderId = 1_000_000;
    measure("new order crossing 3 levels", 200.0, 300_000, move |_| {
        out.clear();
        // Rebuild three ask levels, then sweep all of them.
        for k in 0..3i64 {
            id += 1;
            e.apply_into(
                &Event::New {
                    instrument: I,
                    id,
                    owner: 1,
                    side: Side::Ask,
                    price: 10_000 + k,
                    qty: 10,
                    kind: OrderKind::Limit,
                },
                &mut out,
            );
        }
        id += 1;
        e.apply_into(
            &Event::New {
                instrument: I,
                id,
                owner: 2,
                side: Side::Bid,
                price: 10_002,
                qty: 30,
                kind: OrderKind::Limit,
            },
            &mut out,
        );
        black_box(&out);
    })
}

fn bench_cancel() -> Bench {
    const N: usize = 200_000;
    best_of("cancel", 40.0, 5, || {
        // Untimed: fill a book with resting orders across 50 price levels.
        let mut e = Engine::new();
        let mut out = Vec::with_capacity(64);
        for i in 0..N {
            e.apply_into(
                &Event::New {
                    instrument: I,
                    id: i as OrderId + 1,
                    owner: 1,
                    side: Side::Bid,
                    price: 9_900 + (i as Ticks % 50),
                    qty: 10,
                    kind: OrderKind::Limit,
                },
                &mut out,
            );
        }
        // Timed: cancel them in submission order, which is the head of each
        // level — the common case, since traders cancel their oldest quotes.
        let t = Instant::now();
        for i in 0..N {
            out.clear();
            e.apply_into(
                &Event::Cancel {
                    instrument: I,
                    id: i as OrderId + 1,
                },
                &mut out,
            );
            black_box(&out);
        }
        (t.elapsed(), N)
    })
}

fn bench_aggregate_five_providers() -> Bench {
    let mut e = Engine::new();
    for p in [10u16, 20, 30, 40, 50] {
        snapshot(&mut e, p, 10_000, 10);
    }
    measure("aggregate 5 providers, depth 10", 300.0, 500_000, move |_| {
        black_box(e.aggregate(I, 10));
    })
}

fn bench_top_of_book() -> Bench {
    let mut e = Engine::new();
    for p in [10u16, 20, 30, 40, 50] {
        snapshot(&mut e, p, 10_000, 10);
    }
    measure("top_of_book, 5 providers", 0.0, 2_000_000, move |_| {
        black_box(e.top_of(I));
    })
}

fn bench_trigger_check_idle() -> Bench {
    let mut e = Engine::new();
    snapshot(&mut e, 10, 10_000, 10);
    // A thousand parked triggers, none close to firing. This is what the hot
    // path pays for having conditional orders at all — it should be flat,
    // because the check is four comparisons against four bucket heads.
    for n in 0..1_000u64 {
        e.apply(&Event::PlaceTrigger {
            trigger: Trigger::new(
                n,
                1,
                I,
                if n % 2 == 0 { Side::Bid } else { Side::Ask },
                if n % 2 == 0 {
                    20_000 + n as Ticks
                } else {
                    1_000 - n as Ticks
                },
                10,
                TriggerKind::Stop,
            ),
        });
    }
    let mut out = Vec::with_capacity(64);
    measure("delta with 1000 parked triggers", 0.0, 1_000_000, move |i| {
        out.clear();
        e.apply_into(
            &Event::Delta {
                provider: ProviderId(10),
                instrument: I,
                side: Side::Bid,
                price: 9_995,
                qty: 1 + (i as Lots % 97),
            },
            &mut out,
        );
        black_box(&out);
    })
}

fn bench_mixed_workload() -> Bench {
    // The realistic number: the generated stream from `testkit`, mixing
    // providers, orders, cancels, triggers, ticks and disconnects.
    let events = scripted_events(200_000, 0xB0FFE7);
    let mut out = Vec::with_capacity(1024);
    let mut engine = Engine::new().with_max_age_ns(5_000);
    let mut cursor = 0usize;
    measure("mixed workload (testkit stream)", 0.0, 1_000_000, move |_| {
        if cursor >= events.len() {
            cursor = 0;
            engine = Engine::new().with_max_age_ns(5_000);
        }
        out.clear();
        engine.apply_into(&events[cursor], &mut out);
        cursor += 1;
        black_box(&out);
    })
}
