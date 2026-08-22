pub type Ticks = i64;
pub type Lots = i64;
pub type OrderId = u64;

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Side {
    Bid,
    Ask,
}
impl Side {
    pub const fn opposite(self) -> Self {
        match self {
            Self::Bid => Self::Ask,
            Self::Ask => Self::Bid,
        }
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum OrderKind {
    Market,
    Limit { price: Ticks },
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct NewOrder {
    pub id: OrderId,
    pub side: Side,
    pub qty: Lots,
    pub kind: OrderKind,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum RejectReason {
    DuplicateId,
    ZeroQty,
    NegativeQty,
    BookQuantityOverflow,
    NonPositivePrice,
    UnknownOrder,
    NoLiquidity,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Change {
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
    Trade {
        price: Ticks,
        qty: Lots,
        maker: OrderId,
        taker: OrderId,
        aggressor: Side,
    },
}
