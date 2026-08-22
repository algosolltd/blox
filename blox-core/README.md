# blox-core

A pure single-book matching engine with price-time priority.

The complete mutating interface is:

- `OrderBook::submit(NewOrder)` where `OrderKind` is `Market` or `Limit`;
- `OrderBook::cancel(OrderId)`.

All prices and quantities are integers. Market remainders never rest. Limit
remainders rest at their limit. Trades always execute at the maker price.
