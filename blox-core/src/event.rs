//! Input and output vocabularies. See `docs/CORE.md` §2 and §3.
//!
//! Two event vocabularies, not one: `OrderBook` events carry order identity,
//! `LevelBook` events do not, because the provider never gave us any.

use crate::types::*;
use serde::{Deserialize, Serialize};

/// How an order behaves when it meets the book.
#[derive(Copy, Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub enum OrderKind {
    /// Take what crosses, rest the remainder.
    Limit,
    /// No price limit. Take what's there, drop the remainder.
    Market,
    /// Immediate-or-cancel: take what crosses now, drop the remainder.
    Ioc,
    /// Fill-or-kill: all of it or none. Pre-checked, never partially fills.
    Fok,
    /// Reject outright if it would cross. Guarantees maker status.
    PostOnly,
}

/// Self-trade prevention. See `docs/CORE.md` §5.
#[derive(Copy, Clone, PartialEq, Eq, Debug, Default, Serialize, Deserialize)]
pub enum StpMode {
    /// Allow an owner to trade with themselves. Correct for games and sims.
    #[default]
    None,
    /// Cancel the owner's resting order, keep the aggressor going.
    CancelResting,
    /// Reject the incoming order, leave the book untouched.
    CancelIncoming,
}

/// A conditional order. See `docs/CONDITIONAL_ORDERS.md`.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum TriggerKind {
    /// Fires as the price moves away from you; becomes a market order.
    Stop,
    /// Fires like `Stop`, becomes a limit at `limit_price`.
    StopLimit { limit_price: Ticks },
    /// Fires as the price comes toward you; becomes a limit at the trigger.
    Limit,
}

#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub struct Trigger {
    pub id: OrderId,
    pub owner: OwnerId,
    pub instrument: InstrumentId,
    pub side: Side,
    pub trigger_price: Ticks,
    pub qty: Lots,
    pub kind: TriggerKind,
    /// Set for TP/SL. The app cancels these when the position closes.
    pub attached_to: Option<PositionId>,
    /// Set for TP/SL pairs. Firing one cancels the other.
    pub oco_with: Option<OrderId>,
}

impl Trigger {
    /// Minimal constructor — the common case has no position and no sibling.
    pub fn new(
        id: OrderId,
        owner: OwnerId,
        instrument: InstrumentId,
        side: Side,
        trigger_price: Ticks,
        qty: Lots,
        kind: TriggerKind,
    ) -> Self {
        Self {
            id,
            owner,
            instrument,
            side,
            trigger_price,
            qty,
            kind,
            attached_to: None,
            oco_with: None,
        }
    }

    /// Does this trigger fire as the price rises (`true`) or falls (`false`)?
    ///
    /// Grouping by direction of travel — not by order type — is what makes the
    /// hot-path check two comparisons. See `docs/CONDITIONAL_ORDERS.md` §6.
    #[inline]
    pub fn fires_upward(&self) -> bool {
        match (self.kind, self.side) {
            // Buy stop sits above the market: fires as price rises.
            (TriggerKind::Stop | TriggerKind::StopLimit { .. }, Side::Bid) => true,
            // Sell stop sits below: fires as price falls.
            (TriggerKind::Stop | TriggerKind::StopLimit { .. }, Side::Ask) => false,
            // Buy limit sits below: fires as price falls.
            (TriggerKind::Limit, Side::Bid) => false,
            // Sell limit sits above: fires as price rises.
            (TriggerKind::Limit, Side::Ask) => true,
        }
    }

    /// Is the firing condition already satisfied at `reference`?
    #[inline]
    pub fn satisfied_at(&self, reference: Ticks) -> bool {
        if self.fires_upward() {
            reference >= self.trigger_price
        } else {
            reference <= self.trigger_price
        }
    }
}

/// Everything that can be fed to the engine.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Event {
    // ---- OrderBook: we own it, orders have identity ----
    New {
        instrument: InstrumentId,
        id: OrderId,
        owner: OwnerId,
        side: Side,
        price: Ticks,
        qty: Lots,
        kind: OrderKind,
    },
    Cancel {
        instrument: InstrumentId,
        id: OrderId,
    },
    Amend {
        instrument: InstrumentId,
        id: OrderId,
        qty: Lots,
    },

    // ---- LevelBook: the provider owns it, no identity ----
    Snapshot {
        provider: ProviderId,
        instrument: InstrumentId,
        bids: Vec<(Ticks, Lots)>,
        asks: Vec<(Ticks, Lots)>,
    },
    Delta {
        provider: ProviderId,
        instrument: InstrumentId,
        side: Side,
        price: Ticks,
        qty: Lots,
    },
    /// Provider disconnected. Drops every book they own, atomically.
    /// The most important operation in a multi-provider system — see
    /// `docs/ADAPTERS.md` §2.
    Clear {
        provider: ProviderId,
    },

    // ---- conditional orders ----
    PlaceTrigger {
        trigger: Trigger,
    },
    CancelTrigger {
        instrument: InstrumentId,
        id: OrderId,
    },

    // ---- engine-level ----
    /// The only source of time. See `docs/CORE.md` §9.
    Tick {
        ts_nanos: u64,
    },
}

#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum RejectReason {
    UnknownOrder,
    DuplicateId,
    ZeroQty,
    NegativeQty,
    /// PostOnly that would have crossed.
    WouldCross,
    /// Fok with insufficient depth.
    InsufficientBook,
    SelfTrade,
    /// Market order into an empty book.
    NoLiquidity,
    /// A trigger whose condition is already met. See CONDITIONAL_ORDERS §5.
    WouldFireImmediately,
}

#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum CancelReason {
    ByUser,
    /// The OCO sibling fired.
    Oco,
    /// The position it was attached to closed.
    PositionClosed,
}

/// What a fired trigger becomes. The core does not execute it — see
/// `docs/CONDITIONAL_ORDERS.md` §8.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum FiredAs {
    Market,
    Limit { price: Ticks },
}

/// Everything the engine reports. `apply` **returns** these; it never calls
/// back (`DESIGN.md` D8).
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Change {
    // ---- book mutations ----
    LevelUpdated {
        provider: ProviderId,
        instrument: InstrumentId,
        side: Side,
        price: Ticks,
        qty: Lots,
    },
    BookReplaced {
        provider: ProviderId,
        instrument: InstrumentId,
    },
    BookCleared {
        provider: ProviderId,
        instrument: InstrumentId,
    },
    BookStale {
        provider: ProviderId,
        instrument: InstrumentId,
        age_nanos: u64,
    },

    /// Aggregate top of book changed. Suppressed when identical to the last
    /// one emitted — the highest-leverage filter in the engine.
    TopOfBook {
        instrument: InstrumentId,
        bid: Option<(Ticks, Lots)>,
        ask: Option<(Ticks, Lots)>,
    },

    // ---- order lifecycle ----
    Accepted {
        id: OrderId,
    },
    Rejected {
        id: OrderId,
        reason: RejectReason,
    },
    Filled {
        id: OrderId,
        price: Ticks,
        qty: Lots,
        remaining: Lots,
    },
    Cancelled {
        id: OrderId,
        remaining: Lots,
    },
    /// A resting order's quantity changed in place — a decrease that kept its
    /// queue position, or an increase that lost it. `qty` is the new resting
    /// size. Amending to zero or below cancels instead; that path emits
    /// `Cancelled`, not this.
    Amended {
        id: OrderId,
        qty: Lots,
    },

    /// A match. `price` is always the maker's price.
    Trade {
        instrument: InstrumentId,
        price: Ticks,
        qty: Lots,
        maker: OrderId,
        taker: OrderId,
        aggressor: Side,
    },

    // ---- conditional orders ----
    TriggerPlaced {
        id: OrderId,
    },
    /// `ref_price` is where the market *is*, not where the trigger was. This
    /// is what makes gap-filling at the stale price structurally impossible.
    TriggerFired {
        id: OrderId,
        ref_price: Ticks,
        becomes: FiredAs,
    },
    TriggerCancelled {
        id: OrderId,
        reason: CancelReason,
    },
}
