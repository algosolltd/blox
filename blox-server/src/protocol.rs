use crate::engine::reject_reason;
use blox_core::{Change, OrderKind, Side};
use serde::{Deserialize, Serialize};

pub const SCHEMA_VERSION: u16 = 1;

#[derive(Debug, Deserialize)]
pub struct Request {
    pub schema_version: u16,
    pub request_id: String,
    #[serde(flatten)]
    pub command: Command,
}
#[derive(Debug, Deserialize)]
#[serde(tag = "command", rename_all = "snake_case")]
pub enum Command {
    CreateBook {
        book_id: String,
    },
    Submit {
        book_id: String,
        id: u64,
        side: WireSide,
        qty: i64,
        kind: WireKind,
    },
    Cancel {
        book_id: String,
        id: u64,
    },
    Snapshot {
        book_id: String,
        depth: usize,
    },
    Subscribe {
        book_id: String,
    },
    Ping,
    Check {
        book_id: String,
    },
}
#[derive(Copy, Clone, Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WireSide {
    Buy,
    Sell,
}
#[derive(Copy, Clone, Debug, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum WireKind {
    Market,
    Limit { price: i64 },
}
impl From<WireSide> for Side {
    fn from(v: WireSide) -> Self {
        match v {
            WireSide::Buy => Side::Bid,
            WireSide::Sell => Side::Ask,
        }
    }
}
impl From<WireKind> for OrderKind {
    fn from(v: WireKind) -> Self {
        match v {
            WireKind::Market => OrderKind::Market,
            WireKind::Limit { price } => OrderKind::Limit { price },
        }
    }
}

#[derive(Debug, Serialize)]
pub struct Envelope<'a, T: Serialize> {
    pub schema_version: u16,
    pub request_id: &'a str,
    #[serde(flatten)]
    pub payload: T,
}
#[derive(Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Reply {
    Ok,
    Error {
        message: String,
    },
    Pong,
    Snapshot {
        book_id: String,
        bids: Vec<(i64, i64)>,
        asks: Vec<(i64, i64)>,
    },
}
#[derive(Serialize)]
pub struct Event<'a> {
    pub r#type: &'static str,
    pub book_id: &'a str,
    pub change: WireChange,
}
#[derive(Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum WireChange {
    Accepted {
        id: u64,
    },
    Rejected {
        id: u64,
        reason: String,
    },
    Filled {
        id: u64,
        price: i64,
        qty: i64,
        remaining: i64,
    },
    Cancelled {
        id: u64,
        remaining: i64,
    },
    Trade {
        price: i64,
        qty: i64,
        maker: u64,
        taker: u64,
        aggressor: WireOutSide,
    },
}
#[derive(Serialize)]
#[serde(rename_all = "snake_case")]
pub enum WireOutSide {
    Buy,
    Sell,
}
impl From<&Change> for WireChange {
    fn from(c: &Change) -> Self {
        match c {
            Change::Accepted { id } => Self::Accepted { id: *id },
            Change::Rejected { id, reason } => Self::Rejected {
                id: *id,
                reason: reject_reason(*reason).into(),
            },
            Change::Filled {
                id,
                price,
                qty,
                remaining,
            } => Self::Filled {
                id: *id,
                price: *price,
                qty: *qty,
                remaining: *remaining,
            },
            Change::Cancelled { id, remaining } => Self::Cancelled {
                id: *id,
                remaining: *remaining,
            },
            Change::Trade {
                price,
                qty,
                maker,
                taker,
                aggressor,
            } => Self::Trade {
                price: *price,
                qty: *qty,
                maker: *maker,
                taker: *taker,
                aggressor: if *aggressor == Side::Bid {
                    WireOutSide::Buy
                } else {
                    WireOutSide::Sell
                },
            },
        }
    }
}
pub fn encode<T: Serialize>(request_id: &str, payload: T) -> String {
    serde_json::to_string(&Envelope {
        schema_version: SCHEMA_VERSION,
        request_id,
        payload,
    })
    .expect("serializable")
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn parses_versioned_limit() {
        let r:Request=serde_json::from_str(r#"{"schema_version":1,"request_id":"a","command":"submit","book_id":"x","id":1,"side":"buy","qty":2,"kind":{"kind":"limit","price":100}}"#).unwrap();
        assert!(matches!(
            r.command,
            Command::Submit {
                kind: WireKind::Limit { price: 100 },
                ..
            }
        ));
    }
}
