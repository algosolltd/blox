//! Newline-delimited text protocol.
//!
//! Text, not binary, deliberately: the point of this shim is to let other
//! languages drive the engine and to be debuggable with `nc`. Framing is a
//! newline, fields are space-separated. Parsing is `split_whitespace` and a
//! `match` — no serialization dependency, and nothing to keep in sync with a
//! schema.
//!
//! ```text
//! > NEW 1 100 7 B 10850 25 LIMIT
//! < OK
//! > TOP 1
//! < TOP 1 10850 25 - -
//! ```
//!
//! Prices and quantities are canonical integers on the wire. Decimal strings
//! are an adapter concern (`blox_core::decimal`); putting them here would put
//! a parse on the hot path for no benefit.

use blox_core::*;

pub enum Cmd {
    Event(Event),
    /// One-shot top-of-book query.
    Top(InstrumentId),
    /// One-shot aggregate depth query.
    Book(InstrumentId, usize),
    /// Stream changes for an instrument to this connection.
    Sub(InstrumentId),
    Unsub(InstrumentId),
    /// Suppress per-command `OK` acks. Throughput measurement uses this plus
    /// `Ping` as a barrier; acking every command would measure the ack.
    Quiet(bool),
    /// Suppress order-lifecycle pushes (fills, rejects) to the submitter.
    ///
    /// Separate from `Quiet` because the two needs are independent: a
    /// benchmark pipelining without reading wants neither, while a trading
    /// agent wants its fills but not an `OK` for every command.
    Echo(bool),
    /// Round-trip barrier. Echoed back verbatim.
    Ping(String),
    Stats,
    /// Assert every invariant. Cheap insurance for a fuzzing client.
    Check,
    Close,
}

pub fn parse(line: &str) -> Result<Cmd, String> {
    let mut t = line.split_whitespace();
    let verb = t.next().ok_or("empty line")?;

    // Small helpers so each arm stays one readable expression.
    macro_rules! next {
        ($what:expr) => {
            t.next().ok_or_else(|| format!("missing {}", $what))?
        };
    }
    macro_rules! num {
        ($what:expr) => {
            next!($what)
                .parse()
                .map_err(|_| format!("bad {}", $what))?
        };
    }

    let cmd = match verb.to_ascii_uppercase().as_str() {
        "NEW" => {
            let instrument = InstrumentId(num!("instrument"));
            let id: OrderId = num!("id");
            let owner: OwnerId = num!("owner");
            let side = parse_side(next!("side"))?;
            let price: Ticks = num!("price");
            let qty: Lots = num!("qty");
            let kind = match next!("kind").to_ascii_uppercase().as_str() {
                "LIMIT" => OrderKind::Limit,
                "MARKET" => OrderKind::Market,
                "IOC" => OrderKind::Ioc,
                "FOK" => OrderKind::Fok,
                "POST" | "POSTONLY" => OrderKind::PostOnly,
                other => return Err(format!("bad kind {other}")),
            };
            Cmd::Event(Event::New {
                instrument,
                id,
                owner,
                side,
                price,
                qty,
                kind,
            })
        }
        "CANCEL" => Cmd::Event(Event::Cancel {
            instrument: InstrumentId(num!("instrument")),
            id: num!("id"),
        }),
        "AMEND" => Cmd::Event(Event::Amend {
            instrument: InstrumentId(num!("instrument")),
            id: num!("id"),
            qty: num!("qty"),
        }),
        "SNAP" => {
            let provider = ProviderId(num!("provider"));
            let instrument = InstrumentId(num!("instrument"));
            let bids = parse_levels(next!("bids"), Side::Bid)?;
            let asks = parse_levels(next!("asks"), Side::Ask)?;
            Cmd::Event(Event::Snapshot {
                provider,
                instrument,
                bids,
                asks,
            })
        }
        "DELTA" => Cmd::Event(Event::Delta {
            provider: ProviderId(num!("provider")),
            instrument: InstrumentId(num!("instrument")),
            side: parse_side(next!("side"))?,
            price: num!("price"),
            qty: num!("qty"),
        }),
        "CLEAR" => Cmd::Event(Event::Clear {
            provider: ProviderId(num!("provider")),
        }),
        "TRIGGER" => {
            let instrument = InstrumentId(num!("instrument"));
            let id: OrderId = num!("id");
            let owner: OwnerId = num!("owner");
            let side = parse_side(next!("side"))?;
            let price: Ticks = num!("price");
            let qty: Lots = num!("qty");
            let spec = next!("kind");
            let kind = match spec.to_ascii_uppercase().as_str() {
                "STOP" => TriggerKind::Stop,
                "LIMIT" => TriggerKind::Limit,
                s if s.starts_with("STOPLIMIT:") => TriggerKind::StopLimit {
                    limit_price: s[10..]
                        .parse()
                        .map_err(|_| "bad stop-limit price".to_string())?,
                },
                other => return Err(format!("bad trigger kind {other}")),
            };
            Cmd::Event(Event::PlaceTrigger {
                trigger: Trigger::new(id, owner, instrument, side, price, qty, kind),
            })
        }
        "CANCELTRIG" => Cmd::Event(Event::CancelTrigger {
            instrument: InstrumentId(num!("instrument")),
            id: num!("id"),
        }),
        "TICK" => Cmd::Event(Event::Tick {
            ts_nanos: num!("ts"),
        }),
        "TOP" => Cmd::Top(InstrumentId(num!("instrument"))),
        "BOOK" => Cmd::Book(InstrumentId(num!("instrument")), num!("depth")),
        "SUB" => Cmd::Sub(InstrumentId(num!("instrument"))),
        "UNSUB" => Cmd::Unsub(InstrumentId(num!("instrument"))),
        "QUIET" => Cmd::Quiet(next!("flag") != "0"),
        "ECHO" => Cmd::Echo(next!("flag") != "0"),
        "PING" => Cmd::Ping(t.next().unwrap_or("").to_string()),
        "STATS" => Cmd::Stats,
        "CHECK" => Cmd::Check,
        "CLOSE" | "QUIT" => Cmd::Close,
        other => return Err(format!("unknown command {other}")),
    };
    Ok(cmd)
}

fn parse_side(s: &str) -> Result<Side, String> {
    match s {
        "B" | "b" | "BID" | "bid" => Ok(Side::Bid),
        "S" | "s" | "A" | "a" | "ASK" | "ask" => Ok(Side::Ask),
        other => Err(format!("bad side {other}")),
    }
}

/// `"10850:25,10849:100"`, or `"-"` for an empty side.
///
/// Sorted and deduplicated here rather than trusted, because the adapter
/// contract (`docs/ADAPTERS.md`) requires sorted input and a remote client is
/// not something to take on trust.
fn parse_levels(s: &str, side: Side) -> Result<Vec<(Ticks, Lots)>, String> {
    if s == "-" || s.is_empty() {
        return Ok(Vec::new());
    }
    let mut v = Vec::new();
    for part in s.split(',') {
        let (p, q) = part.split_once(':').ok_or("level needs price:qty")?;
        let price: Ticks = p.parse().map_err(|_| "bad level price".to_string())?;
        let qty: Lots = q.parse().map_err(|_| "bad level qty".to_string())?;
        if qty <= 0 {
            return Err("snapshot levels must be positive".into());
        }
        v.push((price, qty));
    }
    match side {
        Side::Bid => v.sort_unstable_by_key(|l| std::cmp::Reverse(l.0)),
        Side::Ask => v.sort_unstable_by_key(|l| l.0),
    }
    v.dedup_by_key(|l| l.0);
    Ok(v)
}

fn px(v: Option<(Ticks, Lots)>) -> String {
    match v {
        Some((p, q)) => format!("{p} {q}"),
        None => "- -".to_string(),
    }
}

pub fn format_top(inst: InstrumentId, top: &Top) -> String {
    format!("TOP {} {} {}", inst.0, px(top.bid), px(top.ask))
}

pub fn format_book(view: &BookView) -> String {
    let side = |ls: &[AggLevel]| {
        if ls.is_empty() {
            "-".to_string()
        } else {
            ls.iter()
                .map(|l| format!("{}:{}", l.price, l.qty))
                .collect::<Vec<_>>()
                .join(",")
        }
    };
    format!(
        "BOOK {} {} {}",
        view.instrument.0,
        side(&view.bids),
        side(&view.asks)
    )
}

/// Compact one-line encoding of a change. Only what a client needs to act on.
pub fn format_change(c: &Change) -> String {
    match c {
        Change::Trade {
            instrument,
            price,
            qty,
            maker,
            taker,
            aggressor,
        } => format!(
            "EV TRADE {} {price} {qty} {maker} {taker} {}",
            instrument.0,
            if *aggressor == Side::Bid { "B" } else { "S" }
        ),
        Change::Filled {
            id,
            price,
            qty,
            remaining,
        } => format!("EV FILL {id} {price} {qty} {remaining}"),
        Change::Accepted { id } => format!("EV ACK {id}"),
        Change::Rejected { id, reason } => format!("EV REJECT {id} {reason:?}"),
        Change::Cancelled { id, remaining } => format!("EV CANCEL {id} {remaining}"),
        Change::Amended { id, qty } => format!("EV AMEND {id} {qty}"),
        Change::TopOfBook {
            instrument,
            bid,
            ask,
        } => format!("EV TOP {} {} {}", instrument.0, px(*bid), px(*ask)),
        Change::LevelUpdated {
            provider,
            instrument,
            side,
            price,
            qty,
        } => format!(
            "EV LEVEL {} {} {} {price} {qty}",
            provider.0,
            instrument.0,
            if *side == Side::Bid { "B" } else { "S" }
        ),
        Change::BookReplaced {
            provider,
            instrument,
        } => format!("EV SNAP {} {}", provider.0, instrument.0),
        Change::BookCleared {
            provider,
            instrument,
        } => format!("EV CLEARED {} {}", provider.0, instrument.0),
        Change::BookStale {
            provider,
            instrument,
            age_nanos,
        } => format!("EV STALE {} {} {age_nanos}", provider.0, instrument.0),
        Change::TriggerPlaced { id } => format!("EV TRIGPLACED {id}"),
        Change::TriggerFired {
            id,
            ref_price,
            becomes,
        } => format!(
            "EV TRIGFIRED {id} {ref_price} {}",
            match becomes {
                FiredAs::Market => "MARKET".to_string(),
                FiredAs::Limit { price } => format!("LIMIT:{price}"),
            }
        ),
        Change::TriggerCancelled { id, reason } => format!("EV TRIGCANCEL {id} {reason:?}"),
    }
}

/// Which instrument a change belongs to, for subscription routing.
///
/// `None` means "not instrument-scoped" — order lifecycle changes carry an id
/// but not an instrument, so they go to the connection that submitted them.
pub fn change_instrument(c: &Change) -> Option<InstrumentId> {
    match c {
        Change::Trade { instrument, .. }
        | Change::TopOfBook { instrument, .. }
        | Change::LevelUpdated { instrument, .. }
        | Change::BookReplaced { instrument, .. }
        | Change::BookCleared { instrument, .. }
        | Change::BookStale { instrument, .. } => Some(*instrument),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ev(line: &str) -> Event {
        match parse(line).unwrap() {
            Cmd::Event(e) => e,
            _ => panic!("expected an event from {line}"),
        }
    }

    #[test]
    fn parses_a_new_order() {
        assert_eq!(
            ev("NEW 1 100 7 B 10850 25 LIMIT"),
            Event::New {
                instrument: InstrumentId(1),
                id: 100,
                owner: 7,
                side: Side::Bid,
                price: 10850,
                qty: 25,
                kind: OrderKind::Limit,
            }
        );
    }

    #[test]
    fn parses_every_order_kind_and_side_spelling() {
        for (s, want) in [
            ("MARKET", OrderKind::Market),
            ("ioc", OrderKind::Ioc),
            ("Fok", OrderKind::Fok),
            ("POST", OrderKind::PostOnly),
            ("POSTONLY", OrderKind::PostOnly),
        ] {
            match ev(&format!("NEW 1 1 1 S 100 1 {s}")) {
                Event::New { kind, .. } => assert_eq!(kind, want),
                _ => unreachable!(),
            }
        }
        for s in ["B", "b", "BID", "bid"] {
            match ev(&format!("NEW 1 1 1 {s} 100 1 LIMIT")) {
                Event::New { side, .. } => assert_eq!(side, Side::Bid),
                _ => unreachable!(),
            }
        }
    }

    #[test]
    fn snapshot_levels_are_sorted_best_first_regardless_of_input_order() {
        match ev("SNAP 10 1 99:5,101:7,100:6 105:1,103:2") {
            Event::Snapshot { bids, asks, .. } => {
                assert_eq!(bids, vec![(101, 7), (100, 6), (99, 5)]);
                assert_eq!(asks, vec![(103, 2), (105, 1)]);
            }
            _ => unreachable!(),
        }
    }

    #[test]
    fn a_dash_means_an_empty_side() {
        match ev("SNAP 10 1 - 103:2") {
            Event::Snapshot { bids, asks, .. } => {
                assert!(bids.is_empty());
                assert_eq!(asks, vec![(103, 2)]);
            }
            _ => unreachable!(),
        }
    }

    #[test]
    fn parses_trigger_kinds() {
        match ev("TRIGGER 1 5 7 S 10800 10 STOP") {
            Event::PlaceTrigger { trigger } => {
                assert_eq!(trigger.kind, TriggerKind::Stop);
                assert_eq!(trigger.side, Side::Ask);
            }
            _ => unreachable!(),
        }
        match ev("TRIGGER 1 6 7 S 10800 10 STOPLIMIT:10795") {
            Event::PlaceTrigger { trigger } => assert_eq!(
                trigger.kind,
                TriggerKind::StopLimit {
                    limit_price: 10795
                }
            ),
            _ => unreachable!(),
        }
    }

    #[test]
    fn bad_input_is_rejected_with_a_reason() {
        for bad in [
            "",
            "WAT",
            "NEW 1 100 7 X 10850 25 LIMIT",
            "NEW 1 100 7 B 10850 25 NONSENSE",
            "NEW 1 100",
            "NEW a b c B 1 1 LIMIT",
            "SNAP 10 1 99 -",
            "SNAP 10 1 99:0 -",
            "TRIGGER 1 5 7 S 108 10 STOPLIMIT:xx",
        ] {
            assert!(parse(bad).is_err(), "should have rejected {bad:?}");
        }
    }

    #[test]
    fn formats_top_with_missing_sides() {
        let t = Top {
            bid: Some((100, 5)),
            ask: None,
        };
        assert_eq!(format_top(InstrumentId(3), &t), "TOP 3 100 5 - -");
    }

    #[test]
    fn change_encoding_round_trips_through_whitespace_fields() {
        // Every encoding must be a single line with no embedded newlines,
        // or framing breaks.
        let cs = [
            Change::Accepted { id: 1 },
            Change::Trade {
                instrument: InstrumentId(1),
                price: 100,
                qty: 5,
                maker: 1,
                taker: 2,
                aggressor: Side::Bid,
            },
            Change::TriggerFired {
                id: 9,
                ref_price: 50,
                becomes: FiredAs::Limit { price: 51 },
            },
            Change::TopOfBook {
                instrument: InstrumentId(1),
                bid: None,
                ask: Some((7, 8)),
            },
            Change::Amended { id: 3, qty: 4 },
        ];
        for c in cs {
            let s = format_change(&c);
            assert!(!s.contains('\n'), "{s:?} must be one line");
            assert!(s.starts_with("EV "), "{s:?} must be tagged");
        }
    }
}
