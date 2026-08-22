# TCP JSON Lines protocol v1

Connect to `blox-server` over TCP. Every frame is one UTF-8 JSON object
followed by `\n`; partial reads must be buffered until the newline. Multiple
requests may share a connection.

Frames are limited to 64 KiB. `request_id` and `book_id` are limited to 128
bytes, and snapshot depth is limited to 10,000 levels. Exceeding a semantic
limit returns `type:error`; an oversized frame closes the connection.

Every request contains `schema_version: 1`, an opaque string `request_id` and
`command`. Direct replies echo the request ID. Subscription events are
asynchronous and therefore have no request ID.

## Commands

```json
{"schema_version":1,"request_id":"1","command":"create_book","book_id":"BTC-USD"}
{"schema_version":1,"request_id":"2","command":"submit","book_id":"BTC-USD","id":10,"side":"sell","qty":5,"kind":{"kind":"limit","price":10100}}
{"schema_version":1,"request_id":"3","command":"submit","book_id":"BTC-USD","id":11,"side":"buy","qty":2,"kind":{"kind":"market"}}
{"schema_version":1,"request_id":"4","command":"cancel","book_id":"BTC-USD","id":10}
{"schema_version":1,"request_id":"5","command":"snapshot","book_id":"BTC-USD","depth":20}
{"schema_version":1,"request_id":"6","command":"subscribe","book_id":"BTC-USD"}
{"schema_version":1,"request_id":"7","command":"check","book_id":"BTC-USD"}
{"schema_version":1,"request_id":"8","command":"ping"}
```

`side` is `buy` or `sell`. Quantity and price are positive integer lots/ticks.
Only Market, Limit and Cancel belong to this protocol.

## Replies

```json
{"schema_version":1,"request_id":"1","type":"ok"}
{"schema_version":1,"request_id":"5","type":"snapshot","book_id":"BTC-USD","bids":[[10000,12]],"asks":[[10100,3]]}
{"schema_version":1,"request_id":"8","type":"pong"}
{"schema_version":1,"request_id":"x","type":"error","message":"..."}
```

Snapshots contain `[price, aggregate_qty]` pairs in best-to-worst order.
Application errors are returned as `type:error`; malformed framing or a lost
socket must be handled as transport failure by the client.

Core rejections are direct, correlated errors as well as subscription events.
Their stable messages are `duplicate_id`, `zero_qty`, `negative_qty`,
`book_quantity_overflow`, `non_positive_price`, `unknown_order` and
`no_liquidity`. Creating an existing book returns `book exists` and preserves
the existing state. Subscribing to an unknown book returns `unknown book`.

## Subscription events

After `subscribe`, mutations to that book can produce frames such as:

```json
{"type":"event","book_id":"BTC-USD","change":{"kind":"trade","price":10100,"qty":2,"maker":10,"taker":11,"aggressor":"buy"}}
```

Change kinds are `accepted`, `rejected`, `filled`, `cancelled` and `trade`.
Subscribe before requesting the initial snapshot if the client implements its
own snapshot/live reconciliation. The server does not persist events or offer
resume cursors; reconnecting clients must subscribe again and obtain a new
snapshot.

## Minimal shell client

```bash
printf '%s\n' \
  '{"schema_version":1,"request_id":"1","command":"ping"}' |
  nc 127.0.0.1 7070
```
