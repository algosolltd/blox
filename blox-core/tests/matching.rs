//! Matching scenarios. `docs/CORE.md` §10.

use blox_core::*;

const I: InstrumentId = InstrumentId(1);
const ALICE: OwnerId = 1;
const BOB: OwnerId = 2;

fn limit(id: OrderId, owner: OwnerId, side: Side, price: Ticks, qty: Lots) -> Event {
    Event::New {
        instrument: I,
        id,
        owner,
        side,
        price,
        qty,
        kind: OrderKind::Limit,
    }
}

fn kind(
    id: OrderId,
    owner: OwnerId,
    side: Side,
    price: Ticks,
    qty: Lots,
    kind: OrderKind,
) -> Event {
    Event::New {
        instrument: I,
        id,
        owner,
        side,
        price,
        qty,
        kind,
    }
}

/// Apply, assert invariants hold afterwards, return the changes.
fn step(eng: &mut Engine, e: Event) -> Vec<Change> {
    let out = eng.apply(&e);
    eng.check().expect("invariants must hold after every apply");
    out
}

fn trades(cs: &[Change]) -> Vec<(Ticks, Lots, OrderId, OrderId)> {
    cs.iter()
        .filter_map(|c| match c {
            Change::Trade {
                price,
                qty,
                maker,
                taker,
                ..
            } => Some((*price, *qty, *maker, *taker)),
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
fn maker_price_wins_not_the_aggressor_limit() {
    let mut e = Engine::new();
    step(&mut e, limit(1, ALICE, Side::Ask, 100, 10));
    // Bob bids 105 — crosses. He pays Alice's 100, not his own 105.
    let out = step(&mut e, limit(2, BOB, Side::Bid, 105, 10));
    assert_eq!(trades(&out), vec![(100, 10, 1, 2)]);
    assert!(e.order_book(I).unwrap().is_empty());
}

#[test]
fn time_priority_within_a_price_level() {
    let mut e = Engine::new();
    step(&mut e, limit(1, ALICE, Side::Ask, 100, 10));
    step(&mut e, limit(2, BOB, Side::Ask, 100, 10));
    step(&mut e, limit(3, ALICE, Side::Ask, 100, 10));

    // Take 15: order 1 fills in full, order 2 half. Order 3 untouched.
    let out = step(&mut e, limit(4, BOB, Side::Bid, 100, 15));
    assert_eq!(trades(&out), vec![(100, 10, 1, 4), (100, 5, 2, 4)]);
    assert_eq!(e.order_book(I).unwrap().best_ask(), Some((100, 15)));
}

#[test]
fn price_priority_walks_levels_best_first() {
    let mut e = Engine::new();
    step(&mut e, limit(1, ALICE, Side::Ask, 102, 10));
    step(&mut e, limit(2, ALICE, Side::Ask, 100, 10));
    step(&mut e, limit(3, ALICE, Side::Ask, 101, 10));

    let out = step(&mut e, limit(4, BOB, Side::Bid, 102, 25));
    // Cheapest first: 100, then 101, then 102.
    assert_eq!(
        trades(&out),
        vec![(100, 10, 2, 4), (101, 10, 3, 4), (102, 5, 1, 4)]
    );
}

#[test]
fn partial_fill_keeps_queue_position() {
    let mut e = Engine::new();
    step(&mut e, limit(1, ALICE, Side::Ask, 100, 10));
    step(&mut e, limit(2, BOB, Side::Ask, 100, 10));

    // Half-fill order 1.
    step(&mut e, limit(3, BOB, Side::Bid, 100, 4));
    // Order 1 still has 6 and is still first.
    let out = step(&mut e, limit(4, BOB, Side::Bid, 100, 6));
    assert_eq!(trades(&out), vec![(100, 6, 1, 4)]);
    assert_eq!(e.order_book(I).unwrap().best_ask(), Some((100, 10)));
}

#[test]
fn unfilled_remainder_rests_and_becomes_the_new_best() {
    let mut e = Engine::new();
    step(&mut e, limit(1, ALICE, Side::Ask, 100, 10));
    let out = step(&mut e, limit(2, BOB, Side::Bid, 100, 25));
    assert_eq!(trades(&out), vec![(100, 10, 1, 2)]);
    // 15 left, resting as the new best bid.
    assert_eq!(e.order_book(I).unwrap().best_bid(), Some((100, 15)));
    assert_eq!(e.order_book(I).unwrap().best_ask(), None);
}

#[test]
fn non_crossing_orders_both_rest() {
    let mut e = Engine::new();
    step(&mut e, limit(1, ALICE, Side::Bid, 99, 10));
    let out = step(&mut e, limit(2, BOB, Side::Ask, 101, 10));
    assert!(trades(&out).is_empty());
    let ob = e.order_book(I).unwrap();
    assert_eq!(ob.best_bid(), Some((99, 10)));
    assert_eq!(ob.best_ask(), Some((101, 10)));
}

#[test]
fn cancel_removes_exactly_one_order_from_a_level() {
    let mut e = Engine::new();
    for id in 1..=3 {
        step(&mut e, limit(id, ALICE, Side::Ask, 100, 10));
    }
    // Cancel the middle one.
    let out = step(&mut e, Event::Cancel { instrument: I, id: 2 });
    assert!(matches!(
        out[0],
        Change::Cancelled { id: 2, remaining: 10 }
    ));
    assert_eq!(e.order_book(I).unwrap().best_ask(), Some((100, 20)));

    // The survivors keep their order: 1 then 3.
    let out = step(&mut e, limit(9, BOB, Side::Bid, 100, 20));
    assert_eq!(trades(&out), vec![(100, 10, 1, 9), (100, 10, 3, 9)]);
}

#[test]
fn cancelling_the_last_order_removes_the_level() {
    let mut e = Engine::new();
    step(&mut e, limit(1, ALICE, Side::Ask, 100, 10));
    step(&mut e, Event::Cancel { instrument: I, id: 1 });
    assert_eq!(e.order_book(I).unwrap().best_ask(), None);
    assert!(e.order_book(I).unwrap().is_empty());
}

#[test]
fn cancelling_an_unknown_order_is_rejected() {
    let mut e = Engine::new();
    step(&mut e, limit(1, ALICE, Side::Ask, 100, 10));
    let out = step(&mut e, Event::Cancel { instrument: I, id: 42 });
    assert_eq!(rejected(&out), Some(RejectReason::UnknownOrder));
}

#[test]
fn amend_down_keeps_priority_amend_up_loses_it() {
    let mut e = Engine::new();
    step(&mut e, limit(1, ALICE, Side::Ask, 100, 10));
    step(&mut e, limit(2, BOB, Side::Ask, 100, 10));

    // Reduce order 1: still first in the queue.
    step(
        &mut e,
        Event::Amend {
            instrument: I,
            id: 1,
            qty: 5,
        },
    );
    assert_eq!(e.order_book(I).unwrap().best_ask(), Some((100, 15)));
    let out = step(&mut e, limit(3, BOB, Side::Bid, 100, 5));
    assert_eq!(trades(&out), vec![(100, 5, 1, 3)]);

    // Now increase order 2: it goes to the back. Add a third to prove it.
    step(&mut e, limit(4, ALICE, Side::Ask, 100, 7));
    step(
        &mut e,
        Event::Amend {
            instrument: I,
            id: 2,
            qty: 20,
        },
    );
    // Queue is now [4 (7), 2 (20)] — 2 lost its place to 4.
    let out = step(&mut e, limit(5, BOB, Side::Bid, 100, 27));
    assert_eq!(trades(&out), vec![(100, 7, 4, 5), (100, 20, 2, 5)]);
}

#[test]
fn amend_emits_the_new_qty_on_success() {
    let mut e = Engine::new();
    step(&mut e, limit(1, ALICE, Side::Ask, 100, 10));

    let out = step(
        &mut e,
        Event::Amend {
            instrument: I,
            id: 1,
            qty: 4,
        },
    );
    assert!(out.contains(&Change::Amended { id: 1, qty: 4 }));

    let out = step(
        &mut e,
        Event::Amend {
            instrument: I,
            id: 1,
            qty: 9,
        },
    );
    assert!(out.contains(&Change::Amended { id: 1, qty: 9 }));
}

#[test]
fn amend_to_zero_cancels() {
    let mut e = Engine::new();
    step(&mut e, limit(1, ALICE, Side::Ask, 100, 10));
    let out = step(
        &mut e,
        Event::Amend {
            instrument: I,
            id: 1,
            qty: 0,
        },
    );
    assert!(matches!(out[0], Change::Cancelled { id: 1, .. }));
    assert!(e.order_book(I).unwrap().is_empty());
}

// ---- order kinds ---------------------------------------------------------

#[test]
fn market_order_ignores_price_and_takes_everything_available() {
    let mut e = Engine::new();
    step(&mut e, limit(1, ALICE, Side::Ask, 100, 10));
    step(&mut e, limit(2, ALICE, Side::Ask, 200, 10));
    let out = step(&mut e, kind(3, BOB, Side::Bid, 0, 15, OrderKind::Market));
    // Price 0 is ignored: it sweeps both levels.
    assert_eq!(trades(&out), vec![(100, 10, 1, 3), (200, 5, 2, 3)]);
}

#[test]
fn market_order_into_an_empty_book_is_rejected() {
    let mut e = Engine::new();
    let out = step(&mut e, kind(1, BOB, Side::Bid, 0, 10, OrderKind::Market));
    assert_eq!(rejected(&out), Some(RejectReason::NoLiquidity));
}

#[test]
fn market_order_remainder_is_cancelled_not_rested() {
    let mut e = Engine::new();
    step(&mut e, limit(1, ALICE, Side::Ask, 100, 10));
    let out = step(&mut e, kind(2, BOB, Side::Bid, 0, 25, OrderKind::Market));
    assert!(out
        .iter()
        .any(|c| matches!(c, Change::Cancelled { id: 2, remaining: 15 })));
    assert_eq!(e.order_book(I).unwrap().best_bid(), None);
}

#[test]
fn ioc_takes_what_it_can_and_cancels_the_rest() {
    let mut e = Engine::new();
    step(&mut e, limit(1, ALICE, Side::Ask, 100, 10));
    let out = step(&mut e, kind(2, BOB, Side::Bid, 100, 25, OrderKind::Ioc));
    assert_eq!(trades(&out), vec![(100, 10, 1, 2)]);
    assert!(out
        .iter()
        .any(|c| matches!(c, Change::Cancelled { id: 2, remaining: 15 })));
    assert_eq!(e.order_book(I).unwrap().best_bid(), None);
}

#[test]
fn fok_rejects_without_touching_the_book() {
    let mut e = Engine::new();
    step(&mut e, limit(1, ALICE, Side::Ask, 100, 10));
    let before = format!("{:?}", e.order_book(I).unwrap());

    let out = step(&mut e, kind(2, BOB, Side::Bid, 100, 25, OrderKind::Fok));
    assert_eq!(rejected(&out), Some(RejectReason::InsufficientBook));
    assert!(trades(&out).is_empty());
    // Byte-identical: a rejected FOK must leave no trace.
    assert_eq!(before, format!("{:?}", e.order_book(I).unwrap()));
}

#[test]
fn fok_fills_completely_when_the_book_can_cover_it() {
    let mut e = Engine::new();
    step(&mut e, limit(1, ALICE, Side::Ask, 100, 10));
    step(&mut e, limit(2, ALICE, Side::Ask, 101, 10));
    let out = step(&mut e, kind(3, BOB, Side::Bid, 101, 15, OrderKind::Fok));
    assert_eq!(trades(&out), vec![(100, 10, 1, 3), (101, 5, 2, 3)]);
}

#[test]
fn post_only_rejects_a_crossing_order() {
    let mut e = Engine::new();
    step(&mut e, limit(1, ALICE, Side::Ask, 100, 10));
    let out = step(&mut e, kind(2, BOB, Side::Bid, 100, 10, OrderKind::PostOnly));
    assert_eq!(rejected(&out), Some(RejectReason::WouldCross));
    assert_eq!(e.order_book(I).unwrap().best_ask(), Some((100, 10)));
}

#[test]
fn post_only_rests_when_it_does_not_cross() {
    let mut e = Engine::new();
    step(&mut e, limit(1, ALICE, Side::Ask, 100, 10));
    let out = step(&mut e, kind(2, BOB, Side::Bid, 99, 10, OrderKind::PostOnly));
    assert!(trades(&out).is_empty());
    assert_eq!(e.order_book(I).unwrap().best_bid(), Some((99, 10)));
}

// ---- validation ----------------------------------------------------------

#[test]
fn zero_and_negative_quantities_are_rejected() {
    let mut e = Engine::new();
    assert_eq!(
        rejected(&step(&mut e, limit(1, ALICE, Side::Bid, 100, 0))),
        Some(RejectReason::ZeroQty)
    );
    assert_eq!(
        rejected(&step(&mut e, limit(2, ALICE, Side::Bid, 100, -5))),
        Some(RejectReason::NegativeQty)
    );
    assert!(e.order_book(I).unwrap().is_empty());
}

#[test]
fn duplicate_order_id_is_rejected() {
    let mut e = Engine::new();
    step(&mut e, limit(1, ALICE, Side::Bid, 100, 10));
    let out = step(&mut e, limit(1, ALICE, Side::Bid, 100, 10));
    assert_eq!(rejected(&out), Some(RejectReason::DuplicateId));
    assert_eq!(e.order_book(I).unwrap().order_count(), 1);
}

#[test]
fn an_id_is_reusable_once_the_order_is_gone() {
    let mut e = Engine::new();
    step(&mut e, limit(1, ALICE, Side::Bid, 100, 10));
    step(&mut e, Event::Cancel { instrument: I, id: 1 });
    let out = step(&mut e, limit(1, ALICE, Side::Bid, 100, 10));
    assert_eq!(rejected(&out), None);
}

// ---- self-trade prevention -----------------------------------------------

#[test]
fn stp_none_allows_an_owner_to_trade_with_themselves() {
    let mut e = Engine::new();
    step(&mut e, limit(1, ALICE, Side::Ask, 100, 10));
    let out = step(&mut e, limit(2, ALICE, Side::Bid, 100, 10));
    assert_eq!(trades(&out), vec![(100, 10, 1, 2)]);
}

#[test]
fn stp_cancel_resting_evicts_the_makers_order_and_keeps_going() {
    let mut e = Engine::new().with_stp(StpMode::CancelResting);
    step(&mut e, limit(1, ALICE, Side::Ask, 100, 10));
    step(&mut e, limit(2, BOB, Side::Ask, 100, 10));

    // Alice buys: her own order is cancelled, Bob's fills.
    let out = step(&mut e, limit(3, ALICE, Side::Bid, 100, 10));
    assert!(out
        .iter()
        .any(|c| matches!(c, Change::Cancelled { id: 1, remaining: 10 })));
    assert_eq!(trades(&out), vec![(100, 10, 2, 3)]);
    assert!(e.order_book(I).unwrap().is_empty());
}

#[test]
fn stp_cancel_incoming_rejects_the_aggressor_and_leaves_the_book() {
    let mut e = Engine::new().with_stp(StpMode::CancelIncoming);
    step(&mut e, limit(1, ALICE, Side::Ask, 100, 10));
    let out = step(&mut e, limit(2, ALICE, Side::Bid, 100, 10));
    assert_eq!(rejected(&out), Some(RejectReason::SelfTrade));
    assert!(trades(&out).is_empty());
    assert_eq!(e.order_book(I).unwrap().best_ask(), Some((100, 10)));
}

// ---- conservation --------------------------------------------------------

#[test]
fn quantity_is_conserved_across_a_multi_level_sweep() {
    let mut e = Engine::new();
    for (id, price, qty) in [(1u64, 100, 7), (2, 101, 11), (3, 102, 13)] {
        step(&mut e, limit(id, ALICE, Side::Ask, price, qty));
    }
    let out = step(&mut e, limit(4, BOB, Side::Bid, 102, 31));

    let traded: Lots = trades(&out).iter().map(|t| t.1).sum();
    assert_eq!(traded, 31, "all 31 lots must trade");

    // Maker fills and taker fills each independently sum to the traded total.
    let maker_fills: Lots = out
        .iter()
        .filter_map(|c| match c {
            Change::Filled { id, qty, .. } if *id != 4 => Some(*qty),
            _ => None,
        })
        .sum();
    let taker_fills: Lots = out
        .iter()
        .filter_map(|c| match c {
            Change::Filled { id: 4, qty, .. } => Some(*qty),
            _ => None,
        })
        .sum();
    assert_eq!(maker_fills, 31);
    assert_eq!(taker_fills, 31);
    assert!(e.order_book(I).unwrap().is_empty());
}

#[test]
fn a_book_never_crosses_itself() {
    let mut e = Engine::new();
    // Aggressive interleaving that would cross a naive implementation.
    for i in 0..50u64 {
        let side = if i % 2 == 0 { Side::Bid } else { Side::Ask };
        let price = 100 + (i as Ticks % 7) - 3;
        step(&mut e, limit(i, (i % 3) as OwnerId, side, price, 5));
    }
    // `check()` asserts invariant 3 on every step above; assert it explicitly.
    let ob = e.order_book(I).unwrap();
    if let (Some((b, _)), Some((a, _))) = (ob.best_bid(), ob.best_ask()) {
        assert!(b < a, "bid {b} must be below ask {a}");
    }
}
