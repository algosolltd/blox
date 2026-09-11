# blox

Minimal deterministic matching infrastructure.

```text
blox-core    one price-time order book; Market, Limit and Cancel only
blox-server  multi-book TCP server using versioned JSON Lines
```

The core has no dependencies beyond `std`, no I/O, clock, provider state,
floating point, triggers, execution policies or lifecycle orchestration.
Those concerns live in `algolib`, `algorunner` and provider/broker adapters.

```bash
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo run -p blox-server -- 127.0.0.1:7070
```

Example request:

```json
{"schema_version":1,"request_id":"1","command":"create_book","book_id":"EURUSD"}
{"schema_version":1,"request_id":"2","command":"submit","book_id":"EURUSD","id":1,"side":"sell","qty":10,"kind":{"kind":"limit","price":108510}}
```

Documentation:

- [architecture](docs/ARCHITECTURE.md)
- [`blox-core` Rust API](docs/CORE_API.md)
- [TCP protocol](docs/PROTOCOL.md)
- [server operations](docs/OPERATIONS.md)
- [v2 foundations and execution-research status](docs/V2_RESEARCH_STATUS.md)

Proprietary — All Rights Reserved. Copyright (c) 2026 Algorithmica Solutions Ltd.
See [LICENSE](LICENSE). blox may not be used, copied, modified, distributed,
sublicensed or shipped in any project, personal or commercial, without a
separate written commercial license. Licensing enquiries: info@algorithmicasolutions.com.
