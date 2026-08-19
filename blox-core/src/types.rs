//! Primitive types. See `docs/CORE.md` §1.
//!
//! Integers everywhere. There is no `f64` in this crate — not in prices, not
//! in quantities, not in a convenience helper. See [`crate::decimal`] for
//! conversion at the edges.

use serde::{Deserialize, Serialize};

/// Price, in canonical ticks. `1.08501` at tick_scale 5 is `108501`.
pub type Ticks = i64;

/// Quantity, in canonical lots.
pub type Lots = i64;

/// Caller-assigned order identity. Unique per book.
pub type OrderId = u64;

/// Owner of an order — used for self-trade prevention and attribution.
pub type OwnerId = u32;

/// Sequencer stamp. Assigned outside the core.
pub type Seq = u64;

/// Opaque position handle, for triggers attached to a position (TP/SL).
pub type PositionId = u64;

/// Interned instrument symbol. Strings live in the registry, never on the hot
/// path.
#[derive(Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Debug, Serialize, Deserialize)]
pub struct InstrumentId(pub u16);

/// Interned liquidity source.
#[derive(Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Debug, Serialize, Deserialize)]
pub struct ProviderId(pub u16);

/// The book we own ourselves — customer orders, or a game's participants.
/// See `DESIGN.md` D3: internal liquidity is just another provider.
pub const INTERNAL: ProviderId = ProviderId(0);

#[derive(Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Debug, Serialize, Deserialize)]
#[repr(u8)]
pub enum Side {
    Bid,
    Ask,
}

impl Side {
    #[inline]
    pub fn opposite(self) -> Side {
        match self {
            Side::Bid => Side::Ask,
            Side::Ask => Side::Bid,
        }
    }

    /// Does an order with limit price `limit` cross a resting order at
    /// `resting`? A bid crosses when willing to pay at least the ask.
    #[inline]
    pub fn crosses(self, limit: Ticks, resting: Ticks) -> bool {
        match self {
            Side::Bid => limit >= resting,
            Side::Ask => limit <= resting,
        }
    }

    /// Sign of a position taken by aggressing on this side. +1 buy, -1 sell.
    #[inline]
    pub fn sign(self) -> i64 {
        match self {
            Side::Bid => 1,
            Side::Ask => -1,
        }
    }
}

/// Top of book for one instrument, after aggregation.
#[derive(Copy, Clone, PartialEq, Eq, Debug, Default)]
pub struct Top {
    pub bid: Option<(Ticks, Lots)>,
    pub ask: Option<(Ticks, Lots)>,
}

impl Top {
    /// The price a taker on `side` would transact at.
    ///
    /// Buying lifts the ask, selling hits the bid. This is the reference price
    /// for every trigger — see `docs/CONDITIONAL_ORDERS.md` §4.
    #[inline]
    pub fn taker_price(&self, side: Side) -> Option<Ticks> {
        match side {
            Side::Bid => self.ask.map(|(p, _)| p),
            Side::Ask => self.bid.map(|(p, _)| p),
        }
    }

    /// Midpoint, rounded down. Display only — never use it to trigger.
    #[inline]
    pub fn mid(&self) -> Option<Ticks> {
        match (self.bid, self.ask) {
            (Some((b, _)), Some((a, _))) => Some((b + a) / 2),
            _ => None,
        }
    }

    #[inline]
    pub fn spread(&self) -> Option<Ticks> {
        match (self.bid, self.ask) {
            (Some((b, _)), Some((a, _))) => Some(a - b),
            _ => None,
        }
    }
}

/// Per-instrument scaling. Needed to render prices out and to size positions;
/// never used in matching.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub struct Instrument {
    pub id: InstrumentId,
    /// Canonical decimal places for price. The *finest* across all providers
    /// (`DESIGN.md` D16), so adapters only ever multiply.
    pub tick_scale: u32,
    /// Canonical decimal places for quantity.
    pub lot_scale: u32,
    /// Base-currency units per lot. 100_000 for standard FX.
    pub contract_size: i64,
}

impl Instrument {
    pub const fn new(id: u16, tick_scale: u32, lot_scale: u32, contract_size: i64) -> Self {
        Self {
            id: InstrumentId(id),
            tick_scale,
            lot_scale,
            contract_size,
        }
    }

    /// Notional value in quote currency, in cents.
    ///
    /// `i128` intermediate is required, not defensive: one lot of EURUSD
    /// already reaches ~1.1e14, and BTC at size overflows `i64`.
    pub fn notional_cents(&self, qty: Lots, price: Ticks) -> i64 {
        let n = qty as i128 * self.contract_size as i128 * price as i128 * 100i128
            / 10i128.pow(self.lot_scale + self.tick_scale);
        n as i64
    }

    /// P&L in quote currency cents for `qty` (signed) moving from `entry` to
    /// `mark`. Shorts fall out of the sign on `qty`.
    pub fn pnl_cents(&self, qty: Lots, entry: Ticks, mark: Ticks) -> i64 {
        let diff = mark as i128 - entry as i128;
        (diff * qty as i128 * self.contract_size as i128 * 100i128
            / 10i128.pow(self.lot_scale + self.tick_scale)) as i64
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crossing_rules() {
        // A bid at 105 crosses an ask resting at 100.
        assert!(Side::Bid.crosses(105, 100));
        assert!(Side::Bid.crosses(100, 100));
        assert!(!Side::Bid.crosses(99, 100));
        // An ask at 95 crosses a bid resting at 100.
        assert!(Side::Ask.crosses(95, 100));
        assert!(Side::Ask.crosses(100, 100));
        assert!(!Side::Ask.crosses(101, 100));
    }

    #[test]
    fn taker_price_uses_the_side_you_transact_on() {
        let t = Top {
            bid: Some((10840, 5)),
            ask: Some((10860, 5)),
        };
        // Buying lifts the ask.
        assert_eq!(t.taker_price(Side::Bid), Some(10860));
        // Selling hits the bid.
        assert_eq!(t.taker_price(Side::Ask), Some(10840));
        assert_eq!(t.mid(), Some(10850));
        assert_eq!(t.spread(), Some(20));
    }

    #[test]
    fn notional_one_lot_eurusd() {
        // EURUSD: 5dp price, 2dp lots, 100k contract.
        let eurusd = Instrument::new(1, 5, 2, 100_000);
        // 1.00 lot at 1.08500 = $108,500.00 = 10_850_000 cents
        assert_eq!(eurusd.notional_cents(100, 108_500), 10_850_000);
    }

    #[test]
    fn notional_does_not_overflow_on_large_instruments() {
        // BTCUSD: 2dp price, 2dp lots, 1 contract. 10 lots at $100,000.
        let btc = Instrument::new(2, 2, 2, 1);
        assert_eq!(btc.notional_cents(1_000, 10_000_000), 100_000_000);

        // A deliberately extreme case that would overflow an i64 intermediate:
        // qty * contract * price = 1e3 * 1e5 * 1e7 = 1e15, * 100 = 1e17. Fits
        // i128 comfortably; the point is the intermediate never touches i64.
        let big = Instrument::new(3, 2, 2, 100_000);
        assert_eq!(big.notional_cents(1_000, 10_000_000), 10_000_000_000_000);
    }

    #[test]
    fn pnl_sign_follows_quantity() {
        let eurusd = Instrument::new(1, 5, 2, 100_000);
        // Long 1 lot from 1.08500, mark 1.08600 -> +10 pips -> $100
        assert_eq!(eurusd.pnl_cents(100, 108_500, 108_600), 10_000);
        // Short 1 lot from 1.08500, mark 1.08600 -> -$100
        assert_eq!(eurusd.pnl_cents(-100, 108_500, 108_600), -10_000);
    }
}
