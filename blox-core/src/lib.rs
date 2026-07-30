//! # blox-core
//!
//! A pure, deterministic, I/O-free order book and matching engine.
//!
//! No sockets, no clock, no threads, no randomness, no floats. Everything is a
//! function of its inputs, which is what makes it linkable from any language,
//! testable without infrastructure, and replayable after the fact.
//!
//! ```
//! use blox_core::*;
//!
//! let mut eng = Engine::new();
//! let inst = InstrumentId(1);
//! let mut out = Vec::new();
//!
//! // Someone rests an offer.
//! eng.apply_into(&Event::New {
//!     instrument: inst, id: 1, owner: 10,
//!     side: Side::Ask, price: 108_510, qty: 100, kind: OrderKind::Limit,
//! }, &mut out);
//!
//! // Someone lifts it, bidding higher than they need to.
//! out.clear();
//! eng.apply_into(&Event::New {
//!     instrument: inst, id: 2, owner: 20,
//!     side: Side::Bid, price: 108_520, qty: 150, kind: OrderKind::Limit,
//! }, &mut out);
//!
//! // They pay the maker's price (108_510), not their own limit.
//! let trade = out.iter().find_map(|c| match c {
//!     Change::Trade { price, qty, .. } => Some((*price, *qty)),
//!     _ => None,
//! });
//! assert_eq!(trade, Some((108_510, 100)));
//! ```
//!
//! ## Layout
//!
//! | Module | Role |
//! |---|---|
//! | [`types`] | primitives — [`Ticks`], [`Lots`], [`Side`], [`Instrument`] |
//! | [`event`] | the two input vocabularies and the output vocabulary |
//! | [`level_book`] | provider replicas — aggregated depth, no identity |
//! | [`order_book`] | authoritative books — price-time priority |
//! | [`triggers`] | conditional orders — stops, TP/SL, OCO |
//! | [`engine`] | the container, aggregation, change suppression |
//! | [`decimal`] | integer decimal parsing for adapter boundaries |
//! | [`testkit`] | seeded workload generation for tests and benchmarks |
//!
//! ## The rules that must not be broken
//!
//! Violating any of these silently breaks replay, and replay is how everything
//! else gets debugged.
//!
//! - No `std::time`. Time arrives as [`Event::Tick`].
//! - No `f64`. Prices and quantities are integers; see [`decimal`].
//! - No `HashMap` iteration reaching the output. Lookup is fine; ordering is
//!   not. Sort by key, or keep a parallel sorted `Vec`.
//! - No `rand`. Seed a generator in the caller and pass values in as events.
//! - No threads or `async`. The core is `&mut self` and single-owner; the
//!   server sharding by book gives parallelism without locks.
//!
//! See `docs/CORE.md` for the full specification.

pub mod decimal;
pub mod engine;
pub mod event;
pub mod level_book;
pub mod order_book;
pub mod testkit;
pub mod triggers;
pub mod types;

pub use decimal::{format_decimal, parse_decimal, rescale, ParseErr};
pub use engine::{AggLevel, BookView, Engine};
pub use event::{
    CancelReason, Change, Event, FiredAs, OrderKind, RejectReason, StpMode, Trigger, TriggerKind,
};
pub use level_book::LevelBook;
pub use order_book::OrderBook;
pub use triggers::Triggers;
pub use types::{
    Instrument, InstrumentId, Lots, OrderId, OwnerId, PositionId, ProviderId, Seq, Side, Ticks,
    Top, INTERNAL,
};
