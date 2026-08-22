# `blox-server` operations

Run with `cargo run -p blox-server -- 127.0.0.1:7070`. Passing
`127.0.0.1:0` selects an ephemeral port and prints the resolved listening
address, which is useful in integration tests.

The server owns a registry of independent books and funnels all mutations
through one engine thread. Clients have bounded outbound queues; a slow
subscriber must reconnect and resnapshot rather than assuming lossless replay.
The engine ingress queue is also bounded. Frames and identifier lengths are
limited as documented in `PROTOCOL.md`; a framing violation or exhausted
outbound queue closes the socket so reader and writer share one lifecycle.

Operational callers should:

1. establish TCP and create or discover the intended book contract externally;
2. subscribe and take a snapshot;
3. correlate direct responses using `request_id`;
4. treat disconnect as loss of subscription state;
5. reconnect, resubscribe and rebuild from a fresh snapshot;
6. use `check` for diagnostics, not as a high-frequency health endpoint.

There is currently no authentication, TLS, persistence or distributed leader
election. Bind to a private interface or place the service behind an
authenticated transport proxy. Durable command/event logs belong to the
calling execution service.
