use crate::protocol::{Command, Reply};
use blox_core::{Change, NewOrder, OrderBook, RejectReason, Side};
use std::collections::HashMap;

pub const MAX_BOOK_ID_LEN: usize = 128;
pub const MAX_REQUEST_ID_LEN: usize = 128;
pub const MAX_SNAPSHOT_DEPTH: usize = 10_000;

pub struct Execution {
    pub reply: Reply,
    pub events: Option<(String, Vec<Change>)>,
    pub subscribe: Option<String>,
}

impl Execution {
    fn reply(reply: Reply) -> Self {
        Self {
            reply,
            events: None,
            subscribe: None,
        }
    }
}

#[derive(Default)]
pub struct Engine {
    books: HashMap<String, OrderBook>,
}

impl Engine {
    pub fn execute(&mut self, command: Command) -> Execution {
        match command {
            Command::CreateBook { book_id } => {
                if let Err(message) = validate_book_id(&book_id) {
                    return Execution::reply(Reply::Error { message });
                }
                match self.books.entry(book_id) {
                    std::collections::hash_map::Entry::Vacant(entry) => {
                        entry.insert(OrderBook::new());
                        Execution::reply(Reply::Ok)
                    }
                    std::collections::hash_map::Entry::Occupied(_) => {
                        Execution::reply(Reply::Error {
                            message: "book exists".into(),
                        })
                    }
                }
            }
            Command::Submit {
                book_id,
                id,
                side,
                qty,
                kind,
            } => {
                let Some(book) = self.books.get_mut(&book_id) else {
                    return Execution::reply(unknown_book());
                };
                let changes = book.submit(NewOrder {
                    id,
                    side: side.into(),
                    qty,
                    kind: kind.into(),
                });
                let reply = rejection_reply(&changes).unwrap_or(Reply::Ok);
                Execution {
                    reply,
                    events: Some((book_id, changes)),
                    subscribe: None,
                }
            }
            Command::Cancel { book_id, id } => {
                let Some(book) = self.books.get_mut(&book_id) else {
                    return Execution::reply(unknown_book());
                };
                let changes = book.cancel(id);
                let reply = rejection_reply(&changes).unwrap_or(Reply::Ok);
                Execution {
                    reply,
                    events: Some((book_id, changes)),
                    subscribe: None,
                }
            }
            Command::Snapshot { book_id, depth } => {
                if depth > MAX_SNAPSHOT_DEPTH {
                    return Execution::reply(Reply::Error {
                        message: format!("depth exceeds maximum {MAX_SNAPSHOT_DEPTH}"),
                    });
                }
                match self.books.get(&book_id) {
                    Some(book) => Execution::reply(Reply::Snapshot {
                        book_id,
                        bids: book.depth(Side::Bid, depth),
                        asks: book.depth(Side::Ask, depth),
                    }),
                    None => Execution::reply(unknown_book()),
                }
            }
            Command::Subscribe { book_id } => {
                if !self.books.contains_key(&book_id) {
                    return Execution::reply(unknown_book());
                }
                Execution {
                    reply: Reply::Ok,
                    events: None,
                    subscribe: Some(book_id),
                }
            }
            Command::Ping => Execution::reply(Reply::Pong),
            Command::Check { book_id } => match self.books.get(&book_id) {
                None => Execution::reply(unknown_book()),
                Some(book) => match book.check() {
                    Ok(()) => Execution::reply(Reply::Ok),
                    Err(message) => Execution::reply(Reply::Error { message }),
                },
            },
        }
    }
}

fn validate_book_id(book_id: &str) -> Result<(), String> {
    if book_id.is_empty() {
        Err("book_id must not be empty".into())
    } else if book_id.len() > MAX_BOOK_ID_LEN {
        Err(format!("book_id exceeds maximum {MAX_BOOK_ID_LEN} bytes"))
    } else {
        Ok(())
    }
}

fn unknown_book() -> Reply {
    Reply::Error {
        message: "unknown book".into(),
    }
}

fn rejection_reply(changes: &[Change]) -> Option<Reply> {
    changes.iter().find_map(|change| match change {
        Change::Rejected { reason, .. } => Some(Reply::Error {
            message: reject_reason(*reason).into(),
        }),
        _ => None,
    })
}

pub fn reject_reason(reason: RejectReason) -> &'static str {
    match reason {
        RejectReason::DuplicateId => "duplicate_id",
        RejectReason::ZeroQty => "zero_qty",
        RejectReason::NegativeQty => "negative_qty",
        RejectReason::BookQuantityOverflow => "book_quantity_overflow",
        RejectReason::NonPositivePrice => "non_positive_price",
        RejectReason::UnknownOrder => "unknown_order",
        RejectReason::NoLiquidity => "no_liquidity",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::{WireKind, WireSide};

    fn create(book_id: &str) -> Command {
        Command::CreateBook {
            book_id: book_id.into(),
        }
    }

    fn submit(book_id: &str, id: u64, side: WireSide, qty: i64, price: i64) -> Command {
        Command::Submit {
            book_id: book_id.into(),
            id,
            side,
            qty,
            kind: WireKind::Limit { price },
        }
    }

    #[test]
    fn duplicate_create_preserves_existing_book() {
        let mut engine = Engine::default();
        assert!(matches!(engine.execute(create("x")).reply, Reply::Ok));
        engine.execute(submit("x", 1, WireSide::Buy, 2, 100));
        assert!(matches!(
            engine.execute(create("x")).reply,
            Reply::Error { .. }
        ));
        let snapshot = engine.execute(Command::Snapshot {
            book_id: "x".into(),
            depth: 10,
        });
        assert!(matches!(
            snapshot.reply,
            Reply::Snapshot { ref bids, .. } if bids == &vec![(100, 2)]
        ));
    }

    #[test]
    fn rejection_is_a_correlated_error_and_an_event() {
        let mut engine = Engine::default();
        engine.execute(create("x"));
        let result = engine.execute(Command::Cancel {
            book_id: "x".into(),
            id: 7,
        });
        assert!(matches!(
            result.reply,
            Reply::Error { ref message } if message == "unknown_order"
        ));
        assert!(matches!(
            result.events.as_ref(),
            Some((book_id, changes))
                if book_id == "x"
                    && matches!(changes.as_slice(), [Change::Rejected { id: 7, .. }])
        ));
    }

    #[test]
    fn subscribe_requires_an_existing_book() {
        let mut engine = Engine::default();
        assert!(matches!(
            engine
                .execute(Command::Subscribe {
                    book_id: "missing".into()
                })
                .reply,
            Reply::Error { .. }
        ));
    }
}
