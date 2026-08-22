# `blox-core` integration

`blox-core` is an in-process, synchronous Rust matching engine. One
`OrderBook` represents one instrument/book. The caller owns instrument
metadata, persistence, clocks, sequencing and concurrency.

## Dependency

```toml
[dependencies]
blox-core = { path = "../blox/blox-core" }
```

```rust
use blox_core::{Change, NewOrder, OrderBook, OrderKind, Side};

let mut book = OrderBook::new();
let changes = book.submit(NewOrder {
    id: 42,
    side: Side::Bid,
    qty: 10,
    kind: OrderKind::Limit { price: 10_025 },
});
for change in changes {
    match change {
        Change::Trade { price, qty, maker, taker, .. } => {
            println!("{qty}@{price}: {maker} -> {taker}");
        }
        _ => {}
    }
}
```

## Units and identifiers

- `Ticks` and `Lots` are signed `i64`; valid submitted price and quantity are
  positive integers. Scaling is external to the engine.
- `OrderId` is `u64` and must be unique among live orders in that book. An ID
  becomes reusable after the order is fully filled or cancelled.
- `Side::Bid` is buy and `Side::Ask` is sell.
- Market orders never rest. A market order with no opposite liquidity is
  rejected; after one or more fills, an unmatched remainder is cancelled.
- Aggregate quantity at one price level cannot exceed `i64::MAX`. A limit
  order whose resting remainder would exceed it is rejected with
  `BookQuantityOverflow`.

## Mutating API

- `submit(NewOrder) -> Vec<Change>` accepts Market or Limit.
- `submit_into(NewOrder, &mut Vec<Change>)` appends without allocating a new
  result vector.
- `cancel(OrderId) -> Vec<Change>` cancels a live remainder.
- `cancel_into(OrderId, &mut Vec<Change>)` is the append variant.

Changes are emitted in causal order. A matching submission can emit
`Accepted`, one or more `Trade`/`Filled` changes, and a final cancellation for
an unmatched market remainder. Trades execute at the resting maker price and
resting liquidity follows price-time FIFO.

Validation is deterministic: quantity, limit price, duplicate live ID, level
capacity and empty-market liquidity are checked in that order. Rejections do
not mutate the book. `submit_into` and `cancel_into` append to the caller's
existing vector and never clear it.

## Read API

- `best_bid()` / `best_ask()` return `Option<(price, aggregate_qty)>`.
- `depth(side, limit)` returns best-to-worst aggregate levels.
- `order_count()` counts live orders.
- `check()` verifies internal invariants and is useful after recovery/replay.

`cancel` scans the FIFO queue at the indexed price and is therefore linear in
the number of live orders at that price. Best-price and depth reads aggregate
the returned levels and are linear in the number of orders they include.

The engine is deterministic for the same ordered command stream. It is not
internally synchronized; serialize mutation or place each book behind a
single-owner task/thread.
