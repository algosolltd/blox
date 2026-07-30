//! Conditional orders end to end. `docs/CONDITIONAL_ORDERS.md` §11.

use blox_core::*;

const I: InstrumentId = InstrumentId(1);
const LP: ProviderId = ProviderId(10);
const U: OwnerId = 1;

fn snap(bid: Ticks, ask: Ticks) -> Event {
    Event::Snapshot {
        provider: LP,
        instrument: I,
        bids: vec![(bid, 100)],
        asks: vec![(ask, 100)],
    }
}

fn place(id: OrderId, side: Side, price: Ticks, kind: TriggerKind) -> Event {
    Event::PlaceTrigger {
        trigger: Trigger::new(id, U, I, side, price, 10, kind),
    }
}

fn step(eng: &mut Engine, e: Event) -> Vec<Change> {
    let out = eng.apply(&e);
    eng.check().expect("invariants must hold");
    out
}

fn fired(cs: &[Change]) -> Vec<(OrderId, Ticks, FiredAs)> {
    cs.iter()
        .filter_map(|c| match c {
            Change::TriggerFired {
                id,
                ref_price,
                becomes,
            } => Some((*id, *ref_price, *becomes)),
            _ => None,
        })
        .collect()
}

fn rejected(cs: &[Change]) -> Option<RejectReason> {
    cs.iter().find_map(|c| match c {
        Change::Rejected { reason, .. } => Some(*reason),
        _ => None,
    })
}

#[test]
fn buy_stop_fires_on_the_ask_and_ignores_the_mid() {
    let mut e = Engine::new();
    // bid 10800 / ask 10900 -> mid 10850.
    step(&mut e, snap(10_800, 10_900));

    // A buy stop at 10870: the mid (10850) has NOT reached it, but the ask
    // (10900) has. It must fire, because the ask is what you'd pay.
    let out = step(&mut e, place(1, Side::Bid, 10_870, TriggerKind::Stop));
    assert_eq!(rejected(&out), Some(RejectReason::WouldFireImmediately));

    // And one above the ask waits.
    let out = step(&mut e, place(2, Side::Bid, 10_950, TriggerKind::Stop));
    assert!(matches!(out[0], Change::TriggerPlaced { id: 2 }));

    // Market rises: ask reaches 10960.
    let out = step(&mut e, snap(10_940, 10_960));
    assert_eq!(fired(&out), vec![(2, 10_960, FiredAs::Market)]);
}

#[test]
fn sell_stop_fires_on_the_bid_not_the_mid() {
    let mut e = Engine::new();
    step(&mut e, snap(10_800, 10_900));
    // Sell stop at 10750: below the bid, so it waits.
    step(&mut e, place(1, Side::Ask, 10_750, TriggerKind::Stop));

    // Mid falls to 10775 (bid 10750/ask 10800) — the bid has reached 10750.
    let out = step(&mut e, snap(10_750, 10_800));
    assert_eq!(fired(&out), vec![(1, 10_750, FiredAs::Market)]);
}

#[test]
fn a_widening_spread_alone_can_trip_a_sell_stop() {
    // Documented, deliberate behaviour: a stop-loss on a long is a sell, so it
    // references the bid. News or rollover widening will trip it even if the
    // mid never moved.
    let mut e = Engine::new();
    step(&mut e, snap(10_850, 10_860)); // mid 10855
    step(&mut e, place(1, Side::Ask, 10_800, TriggerKind::Stop));

    // Spread widens around an unchanged mid: bid 10790 / ask 10920 -> 10855.
    let out = step(&mut e, snap(10_790, 10_920));
    assert_eq!(e.top_of(I).mid(), Some(10_855), "mid is unchanged");
    assert_eq!(fired(&out), vec![(1, 10_790, FiredAs::Market)]);
}

#[test]
fn a_gap_fills_at_the_market_never_at_the_trigger_price() {
    // The most important behaviour in the file. The market never traded at
    // 10850; reporting that price would invent liquidity nobody could get.
    let mut e = Engine::new();
    step(&mut e, snap(10_890, 10_900));
    step(&mut e, place(1, Side::Ask, 10_850, TriggerKind::Stop));

    // Weekend gap straight through the stop.
    let out = step(&mut e, snap(10_810, 10_820));
    let f = fired(&out);
    assert_eq!(f.len(), 1);
    assert_eq!(f[0].0, 1);
    assert_eq!(f[0].1, 10_810, "ref_price is where the market IS");
    assert_ne!(f[0].1, 10_850, "never where the trigger WAS");
}

#[test]
fn stop_limit_can_gap_past_its_limit_and_simply_not_fill() {
    // Exactly what stop-limit means, and exactly why plain stops exist.
    let mut e = Engine::new();
    step(&mut e, snap(10_890, 10_900));
    step(
        &mut e,
        place(
            1,
            Side::Ask,
            10_850,
            TriggerKind::StopLimit {
                limit_price: 10_845,
            },
        ),
    );

    let out = step(&mut e, snap(10_810, 10_820));
    // It fired, and became a limit at 10845 — which the market is now below.
    assert_eq!(
        fired(&out),
        vec![(1, 10_810, FiredAs::Limit { price: 10_845 })]
    );
}

#[test]
fn a_limit_order_against_a_replica_book_is_also_a_trigger() {
    let mut e = Engine::new();
    step(&mut e, snap(10_800, 10_900));
    // Buy limit below the market: waits for the ask to come down.
    step(&mut e, place(1, Side::Bid, 10_700, TriggerKind::Limit));

    let out = step(&mut e, snap(10_650, 10_690));
    assert_eq!(
        fired(&out),
        vec![(1, 10_690, FiredAs::Limit { price: 10_700 })]
    );
}

#[test]
fn a_trigger_that_would_fire_immediately_is_rejected_not_fired() {
    let mut e = Engine::new();
    step(&mut e, snap(10_800, 10_900));

    // Buy stop below the ask.
    let out = step(&mut e, place(1, Side::Bid, 10_500, TriggerKind::Stop));
    assert_eq!(rejected(&out), Some(RejectReason::WouldFireImmediately));
    assert!(fired(&out).is_empty(), "must never fire on placement");

    // Buy limit above the ask.
    let out = step(&mut e, place(2, Side::Bid, 11_000, TriggerKind::Limit));
    assert_eq!(rejected(&out), Some(RejectReason::WouldFireImmediately));

    assert!(e.triggers_for(I).is_none_or(|t| t.is_empty()));
}

#[test]
fn a_trigger_on_an_empty_book_rests_until_prices_arrive() {
    let mut e = Engine::new();
    // No book at all: no reference price, so nothing fires.
    let out = step(&mut e, place(1, Side::Bid, 100, TriggerKind::Stop));
    assert!(matches!(out[0], Change::TriggerPlaced { id: 1 }));

    let out = step(&mut e, snap(90, 110));
    assert_eq!(fired(&out), vec![(1, 110, FiredAs::Market)]);
}

#[test]
fn oco_siblings_cancel_each_other() {
    let mut e = Engine::new();
    step(&mut e, snap(10_800, 10_900));

    // Classic TP/SL bracket on a long: take profit above, stop loss below.
    let mut tp = Trigger::new(1, U, I, Side::Ask, 11_000, 10, TriggerKind::Limit);
    let mut sl = Trigger::new(2, U, I, Side::Ask, 10_700, 10, TriggerKind::Stop);
    tp.oco_with = Some(2);
    sl.oco_with = Some(1);
    step(&mut e, Event::PlaceTrigger { trigger: tp });
    step(&mut e, Event::PlaceTrigger { trigger: sl });

    // Price rises into the take-profit.
    let out = step(&mut e, snap(11_050, 11_060));
    assert_eq!(
        fired(&out),
        vec![(1, 11_050, FiredAs::Limit { price: 11_000 })]
    );
    // The stop-loss is gone. If it survived it would open a NEW short later.
    assert!(out.iter().any(|c| matches!(
        c,
        Change::TriggerCancelled {
            id: 2,
            reason: CancelReason::Oco
        }
    )));
    assert!(e.triggers_for(I).unwrap().is_empty());
}

#[test]
fn closing_a_position_cancels_every_attached_trigger() {
    let mut e = Engine::new();
    step(&mut e, snap(10_800, 10_900));

    let mut tp = Trigger::new(1, U, I, Side::Ask, 11_000, 10, TriggerKind::Limit);
    let mut sl = Trigger::new(2, U, I, Side::Ask, 10_700, 10, TriggerKind::Stop);
    tp.attached_to = Some(77);
    sl.attached_to = Some(77);
    step(&mut e, Event::PlaceTrigger { trigger: tp });
    step(&mut e, Event::PlaceTrigger { trigger: sl });

    let mut out = Vec::new();
    e.cancel_attached(I, 77, &mut out);
    assert_eq!(
        out,
        vec![
            Change::TriggerCancelled {
                id: 1,
                reason: CancelReason::PositionClosed
            },
            Change::TriggerCancelled {
                id: 2,
                reason: CancelReason::PositionClosed
            },
        ]
    );
    assert!(e.triggers_for(I).unwrap().is_empty());
}

#[test]
fn one_gap_fires_many_stops_in_a_deterministic_order() {
    let mut e = Engine::new();
    step(&mut e, snap(10_900, 10_910));

    // Twenty sell stops scattered below the market, placed out of order.
    for (n, price) in (0..20).map(|n| (n, 10_890 - (n as Ticks * 7) % 137)) {
        step(&mut e, place(100 + n, Side::Ask, price, TriggerKind::Stop));
    }

    // One gap takes out every one of them.
    let out = step(&mut e, snap(10_000, 10_010));
    let f = fired(&out);
    assert_eq!(f.len(), 20);
    // All report the post-gap bid, never their own trigger price.
    assert!(f.iter().all(|(_, refp, _)| *refp == 10_000));
    // Nearest-to-market first, which is a fixed order.
    let prices: Vec<Ticks> = f
        .iter()
        .map(|(id, _, _)| 10_890 - (((*id - 100) as Ticks * 7) % 137))
        .collect();
    let mut sorted = prices.clone();
    sorted.sort_unstable_by(|a, b| b.cmp(a));
    assert_eq!(prices, sorted, "highest sell stop fires first as price falls");
}

#[test]
fn cancelling_a_trigger_works_and_unknown_ids_are_rejected() {
    let mut e = Engine::new();
    step(&mut e, snap(10_800, 10_900));
    step(&mut e, place(1, Side::Bid, 11_000, TriggerKind::Stop));

    let out = step(&mut e, Event::CancelTrigger { instrument: I, id: 1 });
    assert!(matches!(
        out[0],
        Change::TriggerCancelled {
            id: 1,
            reason: CancelReason::ByUser
        }
    ));

    let out = step(&mut e, Event::CancelTrigger { instrument: I, id: 1 });
    assert_eq!(rejected(&out), Some(RejectReason::UnknownOrder));

    // Cancelled means it can no longer fire.
    let out = step(&mut e, snap(11_500, 11_600));
    assert!(fired(&out).is_empty());
}

#[test]
fn a_stale_feed_can_fire_a_trigger_by_changing_the_reference() {
    // Staleness changes the aggregate, which changes the reference price.
    // That is a legitimate reason for a trigger to fire, and it must happen
    // on a Tick like everything else time-driven.
    let mut e = Engine::new().with_max_age_ns(1_000);
    let lp2 = ProviderId(20);

    step(&mut e, Event::Tick { ts_nanos: 0 });
    step(&mut e, snap(10_800, 10_900));
    // lp2 quotes later, so it is still fresh when LP has aged out.
    step(&mut e, Event::Tick { ts_nanos: 900 });
    step(
        &mut e,
        Event::Snapshot {
            provider: lp2,
            instrument: I,
            bids: vec![(10_700, 50)],
            asks: vec![(11_500, 50)],
        },
    );
    // Best ask is LP's 10900.
    assert_eq!(e.top_of(I).ask, Some((10_900, 100)));

    // A buy stop just above it.
    step(&mut e, place(1, Side::Bid, 11_200, TriggerKind::Stop));

    // LP goes quiet (age 1500 > 1000); lp2 is still live (age 600). LP's book
    // leaves the aggregate, so the only ask left is lp2's 11500 — through the
    // stop.
    let out = step(&mut e, Event::Tick { ts_nanos: 1_500 });
    assert_eq!(fired(&out), vec![(1, 11_500, FiredAs::Market)]);
}

#[test]
fn triggers_reject_bad_quantities() {
    let mut e = Engine::new();
    step(&mut e, snap(10_800, 10_900));
    let t = Trigger::new(1, U, I, Side::Bid, 11_000, 0, TriggerKind::Stop);
    assert_eq!(
        rejected(&step(&mut e, Event::PlaceTrigger { trigger: t })),
        Some(RejectReason::ZeroQty)
    );
    let t = Trigger::new(2, U, I, Side::Bid, 11_000, -5, TriggerKind::Stop);
    assert_eq!(
        rejected(&step(&mut e, Event::PlaceTrigger { trigger: t })),
        Some(RejectReason::NegativeQty)
    );
}

#[test]
fn duplicate_trigger_ids_are_rejected() {
    let mut e = Engine::new();
    step(&mut e, snap(10_800, 10_900));
    step(&mut e, place(1, Side::Bid, 11_000, TriggerKind::Stop));
    let out = step(&mut e, place(1, Side::Bid, 11_100, TriggerKind::Stop));
    assert_eq!(rejected(&out), Some(RejectReason::DuplicateId));
    assert_eq!(e.triggers_for(I).unwrap().len(), 1);
}
