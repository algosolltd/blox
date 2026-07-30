//! Determinism. `docs/CORE.md` §9.
//!
//! Violating any determinism rule silently breaks replay, and replay is how
//! everything else gets debugged. These tests are the enforcement.
//!
//! The cross-process half lives in `tests/replay_process.rs`, driving the
//! `replay_hash` binary — `HashMap`'s `RandomState` is seeded per process, so
//! an in-process rerun cannot catch hash-order dependence.

use blox_core::testkit::{hash_changes, scripted_events};
use blox_core::*;

/// The same log, twice, in one process.
#[test]
fn identical_input_produces_identical_output() {
    let events = scripted_events(4_000, 0xDEAD_BEEF);

    let mut a = Engine::new().with_max_age_ns(5_000);
    let mut out_a = Vec::new();
    a.apply_batch(&events, &mut out_a);

    let mut b = Engine::new().with_max_age_ns(5_000);
    let mut out_b = Vec::new();
    b.apply_batch(&events, &mut out_b);

    assert_eq!(out_a.len(), out_b.len(), "change counts differ");
    assert_eq!(out_a, out_b, "change streams differ");
    assert_eq!(hash_changes(&out_a), hash_changes(&out_b));
}

/// Feeding events one at a time must equal feeding them as a batch. If these
/// diverge, some state depends on call boundaries rather than on the events.
#[test]
fn batching_does_not_change_the_result() {
    let events = scripted_events(2_000, 0x1234_5678);

    let mut a = Engine::new().with_max_age_ns(5_000);
    let mut out_a = Vec::new();
    a.apply_batch(&events, &mut out_a);

    let mut b = Engine::new().with_max_age_ns(5_000);
    let mut out_b = Vec::new();
    for e in &events {
        b.apply_into(e, &mut out_b);
    }

    assert_eq!(out_a, out_b);
}

/// A fresh engine replaying a prefix must match the original's prefix — the
/// property that makes "replay to sequence N and look" work.
#[test]
fn replaying_a_prefix_reproduces_that_prefix_exactly() {
    let events = scripted_events(3_000, 0xABCD_1234);

    let mut full = Engine::new().with_max_age_ns(5_000);
    let mut cuts = Vec::new();
    let mut out_full = Vec::new();
    for (i, e) in events.iter().enumerate() {
        full.apply_into(e, &mut out_full);
        if i == 999 || i == 1_999 {
            cuts.push(out_full.len());
        }
    }

    for (n, cut) in [(1_000usize, cuts[0]), (2_000, cuts[1])] {
        let mut part = Engine::new().with_max_age_ns(5_000);
        let mut out_part = Vec::new();
        part.apply_batch(&events[..n], &mut out_part);
        assert_eq!(
            out_part,
            out_full[..cut],
            "replay of the first {n} events diverged"
        );
    }
}

/// Aggregation must not leak `HashMap` iteration order into its output.
/// Registering providers in different orders must produce identical views.
#[test]
fn provider_registration_order_does_not_affect_aggregation() {
    let inst = InstrumentId(1);
    let orders: Vec<Vec<u16>> = vec![
        vec![10, 20, 30, 40, 50],
        vec![50, 40, 30, 20, 10],
        vec![30, 10, 50, 20, 40],
    ];

    let mut views = Vec::new();
    for order in &orders {
        let mut e = Engine::new();
        for &p in order {
            e.apply(&Event::Snapshot {
                provider: ProviderId(p),
                instrument: inst,
                // Deliberately overlapping prices so coalescing is exercised.
                bids: vec![(100, p as Lots), (99, 1)],
                asks: vec![(101, p as Lots), (102, 1)],
            });
        }
        views.push(e.aggregate(inst, 10));
    }

    assert_eq!(views[0], views[1]);
    assert_eq!(views[1], views[2]);
    // And attribution really is ProviderId order, not insertion order.
    let src: Vec<u16> = views[2].bids[0].sources.iter().map(|s| s.0 .0).collect();
    assert_eq!(src, vec![10, 20, 30, 40, 50]);
}

/// Staleness marking iterates books; that iteration must be ordered.
#[test]
fn staleness_reports_books_in_a_stable_order() {
    let run = || {
        let mut e = Engine::new().with_max_age_ns(100);
        e.apply(&Event::Tick { ts_nanos: 0 });
        for p in [70u16, 30, 50, 10, 90] {
            for i in [3u16, 1, 2] {
                e.apply(&Event::Snapshot {
                    provider: ProviderId(p),
                    instrument: InstrumentId(i),
                    bids: vec![(100, 1)],
                    asks: vec![(101, 1)],
                });
            }
        }
        e.apply(&Event::Tick { ts_nanos: 10_000 })
            .into_iter()
            .filter_map(|c| match c {
                Change::BookStale {
                    provider,
                    instrument,
                    ..
                } => Some((provider.0, instrument.0)),
                _ => None,
            })
            .collect::<Vec<_>>()
    };

    let first = run();
    assert_eq!(first.len(), 15);
    let mut sorted = first.clone();
    sorted.sort_unstable();
    assert_eq!(first, sorted, "stale reports must come out sorted");
    for _ in 0..5 {
        assert_eq!(run(), first);
    }
}

/// No `std::time` anywhere: an engine driven only by events must produce the
/// same output no matter how much wall-clock time passes between calls.
#[test]
fn wall_clock_time_has_no_effect() {
    let events = scripted_events(500, 0x5EED);

    let mut a = Engine::new().with_max_age_ns(1_000);
    let mut out_a = Vec::new();
    a.apply_batch(&events, &mut out_a);

    let mut b = Engine::new().with_max_age_ns(1_000);
    let mut out_b = Vec::new();
    for e in &events {
        b.apply_into(e, &mut out_b);
        // Burn real time between events. A clock-reading engine would drift.
        std::hint::black_box(std::time::Instant::now());
    }
    assert_eq!(out_a, out_b);
}
