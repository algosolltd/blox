//! Trading actions produced by agents.

/// The discrete action space dimension used to size model output heads.
/// Combines buy/sell quantity buckets plus hold.
pub fn action_dim() -> usize {
    14
}

#[derive(Clone, Debug, PartialEq)]
pub enum Side {
    Buy,
    Sell,
    Hold,
}

#[derive(Clone, Debug, PartialEq)]
pub enum OrderType {
    Market,
    Limit,
}

#[derive(Clone, Debug)]
pub struct Action {
    pub side: Side,
    pub order_type: OrderType,
    pub price_offset: i64,
    pub qty: u64,
}

impl Action {
    pub fn hold() -> Self {
        Action {
            side: Side::Hold,
            order_type: OrderType::Market,
            price_offset: 0,
            qty: 0,
        }
    }

    pub fn market_buy(qty: u64) -> Self {
        Action {
            side: Side::Buy,
            order_type: OrderType::Market,
            price_offset: 0,
            qty,
        }
    }

    pub fn market_sell(qty: u64) -> Self {
        Action {
            side: Side::Sell,
            order_type: OrderType::Market,
            price_offset: 0,
            qty,
        }
    }

    #[allow(dead_code)]
    pub fn limit_buy(price_offset: i64, qty: u64) -> Self {
        Action {
            side: Side::Buy,
            order_type: OrderType::Limit,
            price_offset,
            qty,
        }
    }

    #[allow(dead_code)]
    pub fn limit_sell(price_offset: i64, qty: u64) -> Self {
        Action {
            side: Side::Sell,
            order_type: OrderType::Limit,
            price_offset,
            qty,
        }
    }

    pub fn sell(_order_id: u64, qty: u64) -> Self {
        Action {
            side: Side::Sell,
            order_type: OrderType::Market,
            price_offset: 0,
            qty,
        }
    }
}

/// Convert a model logit output into an action.
/// Buckets map to buy quantities, sell quantities, or hold.
pub fn action_from_logits(logits: &[f64]) -> Action {
    if logits.is_empty() {
        // No logits available (e.g. malformed model output): hold to stay safe.
        return Action::hold();
    }
    let mut best = 0usize;
    for i in 1..logits.len() {
        if logits[i] > logits[best] {
            best = i;
        }
    }
    const BUY_BUCKETS: usize = 6;
    const SELL_BUCKETS: usize = 6;
    let n = logits.len();
    if best < BUY_BUCKETS.min(n) {
        let qty = (best as u64) + 1;
        Action::market_buy(qty)
    } else if best < (BUY_BUCKETS + SELL_BUCKETS).min(n) {
        let qty = (best - BUY_BUCKETS) as u64 + 1;
        Action::market_sell(qty)
    } else {
        Action::hold()
    }
}