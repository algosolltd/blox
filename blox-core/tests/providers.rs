//! Multi-provider replicas, aggregation, staleness, disconnect.
//! `docs/CORE.md` §6, `docs/ADAPTERS.md` §2.

use blox_core::*;

const EURUSD: InstrumentId = InstrumentId(1);
const GBPUSD: InstrumentId = InstrumentId(2);
const LP_A: ProviderId = ProviderId(10);
const LP_B: ProviderId = ProviderId(20);
const LP_C: ProviderId = ProviderId(30);

fn snap(
    p: ProviderId,
    i: InstrumentId,
    bids: &[(Ticks, Lots)],
    asks: &[(Ticks, Lots)],
) -> Event {
    Event::Snapshot {
        provider: p,
        instrument: i,
        bids: bids.to_vec(),
        asks: asks.to_vec(),
    }
}

fn step(eng: &mut Engine, e: Event) -> Vec<Change> {
    let out = eng.apply(&e);
    eng.check().expect("invariants must hold");
    out
}

type TopPair = (Option<(Ticks, Lots)>, Option<(Ticks, Lots)>);

fn tops(cs: &[Change]) -> Vec<TopPair> {
    cs.iter()
        .filter_map(|c| match c {
            Change::TopOfBook { bid, ask, .. } => Some((*bid, *ask)),
            _ => None,
        })
        .collect()
}

#[test]
fn snapshot_populates_and_reports_top_of_book() {
    let mut e = Engine::new();
    let out = step(
        &mut e,
        snap(LP_A, EURUSD, &[(108_400, 5)], &[(108_600, 7)]),
    );
    assert!(out
        .iter()
        .any(|c| matches!(c, Change::BookReplaced { .. })));
    assert_eq!(
        tops(&out),
        vec![(Some((108_400, 5)), Some((108_600, 7)))]
    );
}

#[test]
fn aggregate_takes_the_best_price_across_providers() {
    let mut e = Engine::new();
    step(&mut e, snap(LP_A, EURUSD, &[(108_400, 5)], &[(108_600, 7)]));
    step(&mut e, snap(LP_B, EURUSD, &[(108_450, 3)], &[(108_650, 9)]));

    let top = e.top_of(EURUSD);
    assert_eq!(top.bid, Some((108_450, 3)), "LP-B has the better bid");
    assert_eq!(top.ask, Some((108_600, 7)), "LP-A has the better ask");
}

#[test]
fn equal_prices_across_providers_sum_their_quantities() {
    let mut e = Engine::new();
    step(&mut e, snap(LP_A, EURUSD, &[(108_400, 5)], &[]));
    step(&mut e, snap(LP_B, EURUSD, &[(108_400, 3)], &[]));
    step(&mut e, snap(LP_C, EURUSD, &[(108_400, 2)], &[]));
    assert_eq!(e.top_of(EURUSD).bid, Some((108_400, 10)));
}

#[test]
fn aggregate_keeps_provider_attribution_sorted_by_id() {
    let mut e = Engine::new();
    // Deliberately register out of order.
    step(&mut e, snap(LP_C, EURUSD, &[(100, 2)], &[]));
    step(&mut e, snap(LP_A, EURUSD, &[(100, 5)], &[]));
    step(&mut e, snap(LP_B, EURUSD, &[(100, 3)], &[]));

    let view = e.aggregate(EURUSD, 5);
    assert_eq!(view.bids.len(), 1);
    assert_eq!(view.bids[0].qty, 10);
    // Sorted by ProviderId, never by size — determinism.
    assert_eq!(
        view.bids[0].sources,
        vec![(LP_A, 5), (LP_B, 3), (LP_C, 2)]
    );
}

#[test]
fn aggregate_merges_depth_and_respects_the_limit() {
    let mut e = Engine::new();
    step(
        &mut e,
        snap(LP_A, EURUSD, &[(100, 1), (98, 1)], &[(101, 1), (103, 1)]),
    );
    step(
        &mut e,
        snap(LP_B, EURUSD, &[(99, 1), (97, 1)], &[(102, 1), (104, 1)]),
    );

    let view = e.aggregate(EURUSD, 3);
    assert_eq!(
        view.bids.iter().map(|l| l.price).collect::<Vec<_>>(),
        vec![100, 99, 98]
    );
    assert_eq!(
        view.asks.iter().map(|l| l.price).collect::<Vec<_>>(),
        vec![101, 102, 103]
    );
}

#[test]
fn an_aggregate_may_legitimately_cross() {
    // LP-A's bid above LP-B's ask is an arbitrage, and real information.
    // Never clamp it.
    let mut e = Engine::new();
    step(&mut e, snap(LP_A, EURUSD, &[(108_700, 5)], &[]));
    step(&mut e, snap(LP_B, EURUSD, &[], &[(108_600, 5)]));

    let top = e.top_of(EURUSD);
    assert_eq!(top.bid, Some((108_700, 5)));
    assert_eq!(top.ask, Some((108_600, 5)));
    assert!(top.bid.unwrap().0 > top.ask.unwrap().0, "crossed, and that is correct");
    e.check().expect("a crossed aggregate is not an invariant breach");
}

#[test]
fn delta_updates_a_level_and_zero_removes_it() {
    let mut e = Engine::new();
    step(&mut e, snap(LP_A, EURUSD, &[(100, 5), (99, 5)], &[]));

    step(
        &mut e,
        Event::Delta {
            provider: LP_A,
            instrument: EURUSD,
            side: Side::Bid,
            price: 100,
            qty: 8,
        },
    );
    assert_eq!(e.top_of(EURUSD).bid, Some((100, 8)));

    step(
        &mut e,
        Event::Delta {
            provider: LP_A,
            instrument: EURUSD,
            side: Side::Bid,
            price: 100,
            qty: 0,
        },
    );
    assert_eq!(e.top_of(EURUSD).bid, Some((99, 5)));
}

#[test]
fn an_identical_delta_produces_no_changes_at_all() {
    let mut e = Engine::new();
    step(&mut e, snap(LP_A, EURUSD, &[(100, 5)], &[]));
    let out = step(
        &mut e,
        Event::Delta {
            provider: LP_A,
            instrument: EURUSD,
            side: Side::Bid,
            price: 100,
            qty: 5,
        },
    );
    assert!(out.is_empty(), "no-op deltas must not reach a consumer");
}

#[test]
fn a_deep_delta_that_does_not_move_the_top_emits_no_top_of_book() {
    let mut e = Engine::new();
    step(&mut e, snap(LP_A, EURUSD, &[(100, 5), (99, 5)], &[]));
    let out = step(
        &mut e,
        Event::Delta {
            provider: LP_A,
            instrument: EURUSD,
            side: Side::Bid,
            price: 99,
            qty: 50,
        },
    );
    // The level updated, but the top of book did not.
    assert!(out
        .iter()
        .any(|c| matches!(c, Change::LevelUpdated { .. })));
    assert!(
        tops(&out).is_empty(),
        "TopOfBook must be suppressed when nothing visible changed"
    );
}

#[test]
fn a_delta_before_any_snapshot_is_dropped() {
    // Debug builds assert; this test documents the release behaviour, so it
    // only runs where the assert is compiled out.
    if cfg!(debug_assertions) {
        return;
    }
    let mut e = Engine::new();
    let out = e.apply(&Event::Delta {
        provider: LP_A,
        instrument: EURUSD,
        side: Side::Bid,
        price: 100,
        qty: 5,
    });
    assert!(out.is_empty());
    assert_eq!(e.dropped_deltas(), 1);
    assert_eq!(e.book_count(), 0);
}

#[test]
fn clear_drops_every_book_a_provider_owns_across_instruments() {
    let mut e = Engine::new();
    step(&mut e, snap(LP_A, EURUSD, &[(100, 5)], &[(102, 5)]));
    step(&mut e, snap(LP_A, GBPUSD, &[(126, 5)], &[(128, 5)]));
    step(&mut e, snap(LP_B, EURUSD, &[(99, 5)], &[(103, 5)]));
    assert_eq!(e.book_count(), 3);

    let out = step(&mut e, Event::Clear { provider: LP_A });

    // Both of LP-A's books are gone, atomically.
    let cleared: Vec<_> = out
        .iter()
        .filter_map(|c| match c {
            Change::BookCleared { instrument, .. } => Some(*instrument),
            _ => None,
        })
        .collect();
    assert_eq!(cleared, vec![EURUSD, GBPUSD]);
    assert_eq!(e.book_count(), 1);

    // LP-B is untouched and now sets the top alone.
    assert_eq!(e.top_of(EURUSD).bid, Some((99, 5)));
    // Nothing is left in GBPUSD at all.
    assert_eq!(e.top_of(GBPUSD), Top::default());
}

#[test]
fn a_stale_book_leaves_the_aggregate_rather_than_poisoning_it() {
    // A dead feed's prices freeze rather than disappear, so they often look
    // like the best price. This is the guard.
    let mut e = Engine::new().with_max_age_ns(1_000);

    step(&mut e, Event::Tick { ts_nanos: 0 });
    step(&mut e, snap(LP_A, EURUSD, &[(108_500, 5)], &[]));
    step(&mut e, Event::Tick { ts_nanos: 500 });
    step(&mut e, snap(LP_B, EURUSD, &[(108_400, 5)], &[]));

    // LP-A shows the better bid.
    assert_eq!(e.top_of(EURUSD).bid, Some((108_500, 5)));

    // Time passes. LP-A last updated at t=0, LP-B at t=500, max age 1000, so
    // t=1200 is the window where LP-A is stale and LP-B is still live.
    let out = step(&mut e, Event::Tick { ts_nanos: 1_200 });
    assert!(out.iter().any(|c| matches!(
        c,
        Change::BookStale {
            provider: LP_A,
            ..
        }
    )));

    // LP-A is excluded; LP-B's worse-but-live price is now the top.
    assert_eq!(e.top_of(EURUSD).bid, Some((108_400, 5)));
}

#[test]
fn an_update_revives_a_stale_book() {
    let mut e = Engine::new().with_max_age_ns(1_000);
    step(&mut e, Event::Tick { ts_nanos: 0 });
    step(&mut e, snap(LP_A, EURUSD, &[(100, 5)], &[]));
    step(&mut e, Event::Tick { ts_nanos: 2_000 });
    assert_eq!(e.top_of(EURUSD).bid, None, "stale, excluded");

    step(&mut e, snap(LP_A, EURUSD, &[(101, 5)], &[]));
    assert_eq!(e.top_of(EURUSD).bid, Some((101, 5)), "fresh again");
}

#[test]
fn staleness_is_only_reported_once_per_book() {
    let mut e = Engine::new().with_max_age_ns(1_000);
    step(&mut e, Event::Tick { ts_nanos: 0 });
    step(&mut e, snap(LP_A, EURUSD, &[(100, 5)], &[]));

    let first = step(&mut e, Event::Tick { ts_nanos: 5_000 });
    assert_eq!(
        first
            .iter()
            .filter(|c| matches!(c, Change::BookStale { .. }))
            .count(),
        1
    );
    let second = step(&mut e, Event::Tick { ts_nanos: 9_000 });
    assert!(second.iter().all(|c| !matches!(c, Change::BookStale { .. })));
}

#[test]
fn internal_orders_and_provider_liquidity_aggregate_together() {
    // DESIGN.md D3: the internal book is just another provider.
    let mut e = Engine::new();
    step(&mut e, snap(LP_A, EURUSD, &[(108_400, 5)], &[(108_600, 5)]));
    step(
        &mut e,
        Event::New {
            instrument: EURUSD,
            id: 1,
            owner: 1,
            side: Side::Bid,
            price: 108_450,
            qty: 3,
            kind: OrderKind::Limit,
        },
    );

    // Our own order is the best bid.
    assert_eq!(e.top_of(EURUSD).bid, Some((108_450, 3)));
    let view = e.aggregate(EURUSD, 5);
    assert_eq!(view.bids[0].sources, vec![(INTERNAL, 3)]);
    assert_eq!(view.bids[1].sources, vec![(LP_A, 5)]);
}
