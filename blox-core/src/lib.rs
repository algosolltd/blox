//! A pure, deterministic, single-book matching engine.
//!
//! The complete mutating interface is `submit` and `cancel`. Orders are either
//! Market or Limit. There is no clock, provider state, policy layer or I/O.

mod book;
mod types;

pub use book::OrderBook;
pub use types::{Change, Lots, NewOrder, OrderId, OrderKind, RejectReason, Side, Ticks};
