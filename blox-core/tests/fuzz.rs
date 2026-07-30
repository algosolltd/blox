//! Property tests over random event streams. `docs/CORE.md` §10.
//!
//! Hand-written tests check the cases you thought of. These check the ones you
//! did not: the linked-list and bookkeeping bugs that need a specific
//! *sequence* rather than a specific input.
//!
//! Every invariant in `docs/CORE.md` §8 is asserted after every single
//! `apply`, across thousands of events and dozens of seeds.

use blox_core::testkit::{scripted_events, Rng};
use blox_core::*;
use std::collections::HashMap;

/// The headline test: all invariants, every event, many seeds.
#[test]
fn invariants_hold_after_every_event() {
    for seed in 1..=40u64 {
        let events = scripted_events(3_000, seed.wrapping_mul(0x9E37_79B9_7F4A_7C15));
        let mut e = Engine::new().with_max_age_ns(2_000);
        let mut out = Vec::new();

        for (i, ev) in events.iter().enumerate() {
            out.clear();
            e.apply_into(ev, &mut out);
            if let Err(msg) = e.check() {
                panic!("seed {seed}, event {i} ({ev:?}): {msg}");
            }
        }
    }
}

/// Same, with self-trade prevention on — it takes a different removal path
/// through the level list and has its own way to corrupt `total`.
#[test]
fn invariants_hold_with_self_trade_prevention() {
    for (mode, name) in [
        (StpMode::CancelResting, "CancelResting"),
        (StpMode::CancelIncoming, "CancelIncoming"),
    ] {
        for seed in 1..=15u64 {
            let events = scripted_events(2_000, seed ^ 0xFEED_FACE);
            let mut e = Engine::new().with_max_age_ns(2_000).with_stp(mode);
            let mut out = Vec::new();
            for (i, ev) in events.iter().enumerate() {
                out.clear();
                e.apply_into(ev, &mut out);
                if let Err(msg) = e.check() {
                    panic!("{name}, seed {seed}, event {i}: {msg}");
                }
            }
        }
    }
}

/// Conservation, per event: what the takers received equals what the makers
/// gave up equals the traded total. `docs/CORE.md` §8, invariants 9 and 10.
#[test]
fn trade_quantity_balances_on_both_sides_of_every_event() {
    for seed in 1..=20u64 {
        let events = scripted_events(4_000, seed ^ 0xC0FF_EE00);
        let mut e = Engine::new();
        let mut out = Vec::new();
        let mut total_traded: Lots = 0;

        for (n, ev) in events.iter().enumerate() {
            out.clear();
            e.apply_into(ev, &mut out);

            let mut traded: Lots = 0;
            let mut per_order: HashMap<OrderId, Lots> = HashMap::new();

            for c in &out {
                match c {
                    Change::Trade {
                        qty, maker, taker, ..
                    } => {
                        assert!(*qty > 0, "seed {seed}, event {n}: non-positive trade");
                        traded += qty;
                        // Each trade must be reflected in both parties' fills.
                        *per_order.entry(*maker).or_default() -= qty;
                        *per_order.entry(*taker).or_default() -= qty;
                    }
                    Change::Filled { id, qty, .. } => {
                        assert!(*qty > 0, "seed {seed}, event {n}: non-positive fill");
                        *per_order.entry(*id).or_default() += qty;
                    }
                    _ => {}
                }
            }

            // Every Trade has exactly one maker Filled and one taker Filled,
            // so the two cancel out for every participant.
            for (id, net) in &per_order {
                assert_eq!(
                    *net, 0,
                    "seed {seed}, event {n}: order {id} has {net} unmatched fill quantity"
                );
            }
            total_traded += traded;
        }

        assert!(
            total_traded > 0,
            "seed {seed}: workload produced no trades, so it proved nothing"
        );
        e.check().unwrap_or_else(|m| panic!("seed {seed}: {m}"));
    }
}

/// A book must never be crossed against itself, no matter the sequence.
#[test]
fn an_order_book_is_never_internally_crossed() {
    for seed in 1..=30u64 {
        let mut r = Rng::new(seed ^ 0xDEAD_10CC);
        let inst = InstrumentId(1);
        let mut e = Engine::new();
        let mut out = Vec::new();

        for id in 0..2_000u64 {
            out.clear();
            let side = if r.below(2) == 0 { Side::Bid } else { Side::Ask };
            // Tight price band so crossings are constant.
            let price = r.range(99, 108);
            let kind = match r.below(8) {
                0 => OrderKind::Market,
                1 => OrderKind::Ioc,
                2 => OrderKind::PostOnly,
                3 => OrderKind::Fok,
                _ => OrderKind::Limit,
            };
            e.apply_into(
                &Event::New {
                    instrument: inst,
                    id,
                    owner: r.below(3) as OwnerId,
                    side,
                    price,
                    qty: r.range(1, 20),
                    kind,
                },
                &mut out,
            );

            let ob = e.order_book(inst).unwrap();
            if let (Some((b, _)), Some((a, _))) = (ob.best_bid(), ob.best_ask()) {
                assert!(b < a, "seed {seed}, order {id}: crossed book bid {b} >= ask {a}");
            }
            e.check().unwrap_or_else(|m| panic!("seed {seed}, order {id}: {m}"));
        }
    }
}

/// A rejected order must leave the book byte-identical. Anything else means a
/// pre-check ran after a mutation.
#[test]
fn rejections_never_mutate_the_book() {
    let mut r = Rng::new(0x9999);
    let inst = InstrumentId(1);
    let mut e = Engine::new();
    let mut out = Vec::new();

    // Build a real book first.
    for id in 0..200u64 {
        e.apply_into(
            &Event::New {
                instrument: inst,
                id,
                owner: (id % 3) as OwnerId,
                side: if id % 2 == 0 { Side::Bid } else { Side::Ask },
                price: if id % 2 == 0 { 95 + (id as Ticks % 5) } else { 105 + (id as Ticks % 5) },
                qty: 10,
                kind: OrderKind::Limit,
            },
            &mut out,
        );
    }

    for n in 0..3_000u64 {
        let before = format!("{:?}", e.order_book(inst).unwrap());
        out.clear();

        // Orders that are guaranteed to be rejected, one way or another.
        let ev = match n % 5 {
            0 => Event::New {
                instrument: inst,
                id: 1_000_000 + n,
                owner: 0,
                side: Side::Bid,
                price: 100,
                qty: 0,
                kind: OrderKind::Limit,
            },
            1 => Event::New {
                instrument: inst,
                id: 1_000_000 + n,
                owner: 0,
                side: Side::Bid,
                price: 100,
                qty: -3,
                kind: OrderKind::Limit,
            },
            2 => Event::New {
                // Duplicate of a live id.
                instrument: inst,
                id: 0,
                owner: 0,
                side: Side::Bid,
                price: 100,
                qty: 5,
                kind: OrderKind::Limit,
            },
            3 => Event::New {
                // FOK far larger than the book.
                instrument: inst,
                id: 1_000_000 + n,
                owner: 0,
                side: Side::Bid,
                price: 200,
                qty: 1_000_000,
                kind: OrderKind::Fok,
            },
            _ => Event::New {
                // PostOnly straight into the ask.
                instrument: inst,
                id: 1_000_000 + n,
                owner: 0,
                side: Side::Bid,
                price: 200,
                qty: r.range(1, 10),
                kind: OrderKind::PostOnly,
            },
        };

        e.apply_into(&ev, &mut out);
        assert!(
            out.iter().any(|c| matches!(c, Change::Rejected { .. })),
            "event {n} should have been rejected: {ev:?}"
        );
        assert_eq!(
            before,
            format!("{:?}", e.order_book(inst).unwrap()),
            "event {n} mutated the book despite being rejected"
        );
    }
}

/// Triggers: nothing may remain parked whose condition is already satisfied.
/// One invariant that covers most of what can go wrong in that module.
#[test]
fn no_trigger_survives_its_own_condition() {
    for seed in 1..=25u64 {
        let events = scripted_events(3_000, seed ^ 0x7A16_6E12);
        let mut e = Engine::new().with_max_age_ns(2_000);
        let mut out = Vec::new();

        for (i, ev) in events.iter().enumerate() {
            out.clear();
            e.apply_into(ev, &mut out);
            for inst in e.instruments() {
                if let Some(t) = e.triggers_for(inst) {
                    let top = e.top_of(inst);
                    t.check(&top)
                        .unwrap_or_else(|m| panic!("seed {seed}, event {i}: {m}"));
                }
            }
        }
    }
}

/// A fired trigger's reported reference price must be a price that actually
/// existed at that moment — never the trigger's own level.
#[test]
fn a_fired_trigger_reports_a_real_market_price() {
    for seed in 1..=20u64 {
        let events = scripted_events(3_000, seed ^ 0x6A9F_0011);
        let mut e = Engine::new().with_max_age_ns(2_000);
        let mut out = Vec::new();

        for ev in &events {
            out.clear();
            e.apply_into(ev, &mut out);

            for c in &out {
                if let Change::TriggerFired { id, ref_price, .. } = c {
                    // The reference must equal a live top-of-book price on the
                    // side the order transacts on, for some instrument.
                    let ok = e.instruments().iter().any(|i| {
                        let t = e.top_of(*i);
                        t.bid.map(|(p, _)| p) == Some(*ref_price)
                            || t.ask.map(|(p, _)| p) == Some(*ref_price)
                    });
                    assert!(
                        ok,
                        "seed {seed}: trigger {id} fired at {ref_price}, which is not a live price"
                    );
                }
            }
        }
    }
}
