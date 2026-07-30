# Adapters — Implementation Spec

> **Design spec, not shipped code.** The `Adapter` trait, `WsAdapter`,
> `SimAdapter`, `ReplayAdapter`, and the Python/Node bindings in §5 don't exist
> in this repo yet — `blox-sim`/`blox-visualize` (Go, over the socket) are the
> only adapters actually running today. What's built: `docs/IMPLEMENTATION.md`
> §7.

An adapter is anything that produces `Event`s. That is the whole definition.

`blox-core` has no I/O, no clock, and no idea where events come from. So a live
FX broker, a recorded log, a synthetic price generator, and a browser game are
all the same kind of thing to it: an event source. They differ only in what
they're plugged into and how much they're allowed to lie.

This is the payoff for D7. There is one pipeline, and everything below feeds it:

```
  ┌──────────────┐
  │ Live LP      │─┐
  ├──────────────┤ │
  │ Replay file  │─┤
  ├──────────────┤ ├──▶ Sequencer ──▶ Engine.apply_into ──▶ [Change] ──▶ consumers
  │ Sim / bots   │─┤     (stamps)      (pure, D4)
  ├──────────────┤ │
  │ Game / users │─┘
  └──────────────┘
```

Because a backtest and a live session traverse identical code from the sequencer
onward, "it worked in the backtest" means something. That property is the single
biggest reason to keep I/O out of the core, and it's worth more than the
performance argument.

---

## 1. The contract

```rust
pub struct Stamped {
    pub seq: Seq,          // monotonic, assigned by the sequencer
    pub recv_ns: u64,      // when WE received it
    pub src_ns: Option<u64>,  // provider's own timestamp, if they send one
    pub event: Event,
}

pub trait Adapter: Send + 'static {
    fn provider(&self) -> ProviderId;

    /// Take ownership, run until cancelled. Emits UNSTAMPED events;
    /// the sequencer stamps them on arrival.
    fn spawn(self: Box<Self>, out: mpsc::Sender<Event>) -> JoinHandle<()>;
}
```

That's the entire interface. Four kinds of adapter implement it and nothing else
in the system needs to distinguish between them.

**Keep `src_ns` separate from `recv_ns` and never conflate them.** The
provider's clock is not your clock — it drifts, it's in a different datacenter,
and some providers stamp at send while others stamp at match. `recv_ns` is what
staleness and sequencing use because it's the only one you can trust. `src_ns`
is diagnostic: `recv_ns - src_ns` is your one-way latency estimate to that LP,
which is genuinely useful for routing and completely useless for ordering.

### Why the sequencer stamps, not the adapter

Adapters run concurrently, one task per provider. If each stamped its own `seq`,
you'd have `k` independent counters and no total order. The sequencer is a
single consumer of one channel, assigning `seq` in the order events physically
arrive.

That order is arbitrary — LP-A and LP-B race on the wire, and nothing can make
that deterministic. The design doesn't try. It records the arbitrary decision
once and makes everything downstream a pure function of the record (D4). The
non-determinism is real; it's confined to one place and written down.

---

## 2. Live provider adapters

The messy layer. Everything a broker does wrong is contained here, so that no
broker's weirdness can reach book logic.

### Responsibilities

| # | Job | Failure if skipped |
|---|---|---|
| 1 | Connect, authenticate, reconnect with backoff | reconnect storms; some LPs ban you |
| 2 | Subscribe to the configured instruments | silence you'll mistake for a quiet market |
| 3 | Parse the provider's dialect | — |
| 4 | Map their symbol → `InstrumentId` (D15) | wrong instrument, silently |
| 5 | Rescale price → canonical ticks (D16) | **routing is wrong and looks right** |
| 6 | Detect sequence gaps → request a snapshot | book drifts from reality permanently |
| 7 | Emit `Snapshot` on connect and after any gap | book starts with a hole |
| 8 | **Emit `Clear` on disconnect** | you route to liquidity that no longer exists |
| 9 | Track and expose reject rate (D19) | routing keeps favouring an LP that never fills |

**#8 is the one people forget and it's the expensive one.** When a socket drops,
that provider's prices are instantly fiction. If you don't clear them, they sit
in the aggregate at whatever they were when the connection died, and because a
dead feed's prices are frozen rather than absent, they often look like the
*best* price in the book — so your router will preferentially send every single
order to the provider that is definitively not there.

Emit `Clear` from the disconnect path itself, not from a health check. A health
check is a timer that can be late; the disconnect is an event you already have.

### Sequence gap handling

Nearly every provider numbers their updates. The pattern is the same everywhere
and it belongs in the adapter, because only the adapter knows the dialect:

```rust
fn on_update(&mut self, u: ProviderUpdate, out: &Sender<Event>) {
    if let Some(seq) = u.seq {
        if let Some(last) = self.last_seq {
            if seq != last + 1 {
                // Gap. Everything we hold for this provider is suspect.
                self.request_snapshot(u.symbol);
                self.awaiting_snapshot.insert(u.symbol);
                self.last_seq = None;
                return;                     // drop the update
            }
        }
        self.last_seq = Some(seq);
    }
    if self.awaiting_snapshot.contains(&u.symbol) { return; }   // drop until resync
    out.send(self.to_event(u)).ok();
}
```

Two rules that matter more than the code:

- **Drop everything for that instrument until the snapshot lands.** Applying
  deltas across a gap produces a book that is subtly wrong and looks completely
  normal. A missing book is obvious; a wrong book is not.
- **A gap is not an error.** Feeds gap routinely — it's a normal operating
  condition, and resync should be silent and cheap. Log it as a counter, not as
  an error, or you will train yourself to ignore the error log.

### Decimal parsing — do not go through `f64`

The single most likely place to reintroduce the bug D10 exists to prevent.
Providers send prices as JSON strings or FIX fields like `"1.08501"`. The
tempting one-liner is fatal:

```rust
let ticks = (s.parse::<f64>()? * 100_000.0) as i64;   // WRONG
```

`1.08501_f64 * 100000.0` is `108500.99999999999`, and `as i64` truncates toward
zero, giving **108500**. One tick low, on some prices and not others, depending
on the bit pattern. It will pass every test you write by hand and fail in
production on the prices you didn't think to try.

Parse the digits directly. No float ever exists:

```rust
/// "1.08501" @ scale 5 -> 108501
/// "1.0850"  @ scale 5 -> 108500   (padded)
/// "-0.5"    @ scale 2 -> -50
pub fn parse_decimal(s: &str, scale: u32) -> Result<i64, ParseErr> {
    let (neg, s) = match s.as_bytes().first() {
        Some(b'-') => (true, &s[1..]),
        Some(b'+') => (false, &s[1..]),
        _ => (false, s),
    };
    let (int, frac) = s.split_once('.').unwrap_or((s, ""));
    if int.is_empty() && frac.is_empty() { return Err(ParseErr::Empty); }

    let mut v: i64 = 0;
    for b in int.bytes() {
        let d = (b as char).to_digit(10).ok_or(ParseErr::BadDigit)?;
        v = v.checked_mul(10).and_then(|v| v.checked_add(d as i64)).ok_or(ParseErr::Overflow)?;
    }
    for i in 0..scale as usize {
        let d = frac.as_bytes().get(i)
            .map(|b| (*b as char).to_digit(10).ok_or(ParseErr::BadDigit))
            .transpose()?
            .unwrap_or(0);                          // pad short fractions
        v = v.checked_mul(10).and_then(|v| v.checked_add(d as i64)).ok_or(ParseErr::Overflow)?;
    }
    // Extra precision beyond our scale: reject rather than silently truncate.
    if frac.len() > scale as usize && frac.as_bytes()[scale as usize..].iter().any(|b| *b != b'0') {
        return Err(ParseErr::TooPrecise);
    }
    Ok(if neg { -v } else { v })
}
```

**Reject excess precision, don't truncate it.** If a provider sends `1.085015`
when your registry says 5 decimals, your registry is wrong. Truncating hides
that permanently, and hides it in the direction that makes your prices look
better than they are. Failing loudly gets it fixed in an afternoon.

### Rescaling between providers (D16)

```rust
// registry says: canonical EURUSD tick_scale = 5, lp_b price_scale = 4
let raw = parse_decimal(msg.price, cfg.price_scale)?;        // 10850
let ticks = raw * 10i64.pow(canonical_scale - cfg.price_scale);   // 108500
```

`canonical_scale` is the **finest** scale across all providers, so this is always
a multiplication and always exact. Assert it at registry load:

```rust
assert!(cfg.price_scale <= canonical_scale,
        "{}: provider scale {} exceeds canonical {} — canonical must be the finest",
        symbol, cfg.price_scale, canonical_scale);
```

Getting this backwards means dividing, which discards the last digit of your
most precise LP — quietly, and always in the direction that makes their price
look worse. Your best provider would systematically lose routing decisions to
worse ones and nothing would look broken.

### Skeleton

```rust
pub struct WsAdapter {
    provider: ProviderId,
    cfg: ProviderCfg,             // url, credentials, symbol map, scales
    last_seq: Option<u64>,
    awaiting: HashSet<InstrumentId>,
}

impl Adapter for WsAdapter {
    fn provider(&self) -> ProviderId { self.provider }

    fn spawn(mut self: Box<Self>, out: mpsc::Sender<Event>) -> JoinHandle<()> {
        tokio::spawn(async move {
            let mut backoff = Duration::from_millis(100);
            loop {
                match self.session(&out).await {
                    Ok(()) => backoff = Duration::from_millis(100),
                    Err(e) => tracing::warn!(?e, provider = ?self.provider, "session ended"),
                }
                // The book is fiction the moment the socket is gone.
                let _ = out.send(Event::Clear { provider: self.provider }).await;
                self.last_seq = None;
                self.awaiting.clear();

                tokio::time::sleep(backoff).await;
                backoff = (backoff * 2).min(Duration::from_secs(30));
            }
        })
    }
}
```

Exponential backoff capped at 30s, reset on a clean session. Reconnecting in a
tight loop after an auth failure gets your IP blocked by most LPs, and that is a
phone call, not a code fix.

### Per-provider config

```toml
[providers.lp_b]
kind      = "websocket"
url       = "wss://lp-b.example/stream"
heartbeat_ms = 5000
max_age_ms   = 500        # feed older than this -> stale, excluded from aggregate

[providers.lp_b.instruments]
EURUSD = { symbol = "EUR/USD", price_scale = 4, lot_scale = 2 }
GBPUSD = { symbol = "GBP/USD", price_scale = 4, lot_scale = 2 }
```

`max_age_ms` is per provider because they differ enormously — a streaming LP
quoting continuously should be considered stale after ~500 ms of silence, while
a request-for-quote provider may legitimately go quiet for minutes.

---

## 3. Replay adapter — backtesting

Reads a recorded log and emits it. Twelve lines, and it's the reason the whole
design holds together.

```rust
pub struct ReplayAdapter { path: PathBuf, speed: Speed }

pub enum Speed {
    Fast,              // as fast as the consumer accepts — backtests
    Realtime,          // honour original inter-event gaps — UI demos
    Scaled(f64),       // 10x, 0.1x — debugging a specific minute
}
```

Because the recorded log is `Stamped` — including the `Tick` events — replay
reproduces staleness marking and expiry exactly as they happened live. No
special casing, no "backtest mode" flag inside the engine. The engine cannot
tell the difference, which is precisely the property you want.

**Where a backtest still lies, and it's worth being honest about it:** replay
reproduces the *market*, not the market's *reaction to you*. Your orders in a
backtest don't move prices, don't consume the liquidity you're filling against,
and don't make an LP widen their quotes because your flow looks toxic. For
passive strategies at small size that error is tolerable. For anything
aggressive or sizeable, backtested fills are optimistic and you should assume
they're optimistic by more than you'd like.

```
// ponytail: no market impact model. Correct for small passive flow. If size
// grows, add a fill probability derived from measured live reject rates —
// don't build a full simulator.
```

### Recording

Recording is the same log the intent journal already writes (D13), so there is
nothing extra to build:

```rust
// length-prefixed, little-endian, append-only
[u32 len][bincode(Stamped)][u32 len][bincode(Stamped)]...
```

Length-prefixed framing means a partially-written final record after a crash is
detectable — you read `len`, find fewer bytes, and truncate there. A
newline-delimited JSON log gives you a half-line you have to guess about.

Rotate hourly, `zstd` the closed files. FX book data compresses to roughly a
tenth of its size because consecutive snapshots are nearly identical.

---

## 4. Sim adapter — synthetic markets

Generates plausible prices with no broker. Used for development, for load
testing, and as the market that a game trades against.

```rust
pub struct SimAdapter {
    provider: ProviderId,
    rng: SmallRng,          // SEEDED — determinism (D4)
    instruments: Vec<SimInstrument>,
}

pub struct SimInstrument {
    id: InstrumentId,
    mid: Ticks,
    spread: Ticks,
    volatility: Ticks,      // std dev of the per-step random walk
    depth: usize,
    step_ns: u64,
}
```

Each step: perturb `mid` by a random walk, rebuild `depth` levels either side of
`mid ± spread/2`, emit a `Snapshot`.

**The seed lives in the adapter, never in the core.** The core has no `rand`
(D4). A seeded generator outside it means "reproduce the exact market that broke
it" is `--seed 42`, which turns an intermittent bug report into a deterministic
one.

Worth adding, because they're each a few lines and they catch different bugs:

- **Occasional gaps** — skip a sequence number, verify resync works
- **Occasional disconnects** — verify `Clear` and reconnect
- **A stale period** — stop quoting without disconnecting, verify staleness
  marking excludes the book
- **Rejects** — for the execution path (D18), a configurable reject rate

These are the failure modes that only show up in production at 3am. Making them
happen on demand in a test is the difference between finding them now and
finding them then.

---

## 5. Game adapters — the fun one

A game is an event source with no network, feeding an `OrderBook` rather than a
`LevelBook`. It uses `New` / `Cancel` / `Amend` and never touches the provider
vocabulary. Roughly half the core is invisible to it — which is the sign the
seam from D11 is in the right place.

### Two ways to build one

**(a) Embedded — Python or Node links the library directly.** No server, no
network, no Postgres. Fastest path, and faster at runtime too, since there's no
socket hop.

**(b) Over the server — game logic is a WS/HTTP client.** Needed for multiple
players sharing one book, or a browser front end.

Start with (a). Move to (b) when a second player appears.

### Embedded, in Python

```python
import blox

eng = blox.Engine()
EURUSD = eng.instrument("EURUSD", tick_scale=5, lot_scale=2)

# Two market-making bots quote around a fair value.
for i, (side, px, qty) in enumerate([
    ("bid", "1.08490", "1.00"),
    ("bid", "1.08480", "2.00"),
    ("ask", "1.08510", "1.00"),
    ("ask", "1.08520", "2.00"),
]):
    eng.new_order(EURUSD, id=i, owner=BOT, side=side, price=px, qty=qty, kind="limit")

# The player lifts the offer.
changes = eng.new_order(EURUSD, id=100, owner=PLAYER,
                        side="bid", price="1.08510", qty="1.50", kind="limit")

for c in changes:
    print(c)
# Trade   { price: 1.08510, qty: 1.00, maker: 2, taker: 100 }
# Filled  { id: 2,   qty: 1.00, remaining: 0 }
# Filled  { id: 100, qty: 1.00, remaining: 0.50 }
# TopOfBook { bid: (1.08510, 0.50), ask: (1.08520, 2.00) }
```

Real price-time priority, real partial fills, exact integer arithmetic — in a
Python script, with no infrastructure.

Note the player's remaining 0.50 became the new best **bid** at 1.08510: it
crossed, took what was there, and rested the remainder. That's genuine limit
order behaviour, not an approximation, and getting it right for free is the
entire argument for reusing the real engine in a game.

**Prices cross the binding as strings, not floats.** `"1.08510"` is parsed by
the same `parse_decimal` the live adapters use. Accepting a Python `float` would
reintroduce D10 at the one boundary where it's least visible — and Python's
`float` is exactly the `f64` we banned. The binding should refuse floats
outright rather than convert them.

### Bots as adapters

For a game with a live market, bots are just another adapter emitting `New` and
`Cancel`. Three archetypes cover most of what a game needs:

| Bot | Behaviour | Teaches the player |
|---|---|---|
| **Market maker** | quotes both sides around fair value, re-quotes on move | spreads exist, and liquidity is a service |
| **Noise trader** | random market orders, random size | the book refills; impact is temporary |
| **Momentum** | buys strength, sells weakness | trends, and how they end |

Together they produce a book that feels alive: a spread that widens under
pressure, depth that gets consumed and rebuilt, and prices that trend and mean-
revert. A random-walk price alone feels dead because nothing reacts to anything.

```
// ponytail: three bots, no calibration to real market microstructure. Enough
// for a game to feel right. If it's a training tool where realism matters,
// fit the parameters to recorded data instead of guessing them.
```

### Game mode differences

| | Game | Live trading |
|---|---|---|
| Books | `OrderBook` only | `LevelBook` per provider + internal `OrderBook` |
| STP | `None` | `CancelResting` |
| Risk gate | off | all five checks + kill switch |
| Intent log | off | required |
| Reconciliation | none | on every startup |
| Persistence | none — restart is free | positions and working orders are real |
| Clock | `Tick` from a game loop | `Tick` from the sequencer |

Same core, same matching, same arithmetic. What differs is entirely in the
layers above it — which is what makes a game a genuinely useful test of the
engine rather than a toy version of it.

---

## 6. Writing a new provider adapter

Order matters. Each step is verifiable before the next, and steps 1–3 need no
network at all.

1. **Get their docs and a sample capture.** Record raw messages to a file before
   writing any parsing code. Every provider's docs are wrong somewhere, and the
   capture is what tells you where.
2. **Write the parser against the capture.** Pure function, `&[u8] → Vec<Event>`.
   Unit-testable with no network and no async.
3. **Fill in the registry** — symbols and scales. Verify `price_scale <=
   canonical_scale` for every instrument, and confirm the scale against the
   capture rather than the docs.
4. **Connection and reconnection.** Get `Clear`-on-disconnect right before
   anything else; it's the expensive one to miss.
5. **Sequence and gap handling.** Test by dropping messages from the capture and
   asserting a snapshot is requested.
6. **Cross-check against a second provider.** Run both live and compare
   top-of-book. Persistent disagreement by a constant factor is a scale bug;
   disagreement by a constant offset is a symbol mapping bug. This one test
   catches nearly every normalization error, and it catches them in minutes.
7. **Execution.** Order out, ack, reject, fill. Test rejects deliberately —
   send a deliberately unfillable price and confirm the reject path works.
8. **Reject rate tracking**, feeding routing (D19).

Step 6 is worth the effort it sounds like. Scale and mapping bugs don't crash,
don't log, and don't look wrong in isolation — they look like an LP with
suspiciously good prices, which is exactly what you were hoping to find.

---

## 7. Adapter checklist

Before an adapter goes near real money:

- [ ] Parses its captured sample with zero errors
- [ ] `parse_decimal` used everywhere; **no `f64` in the file at all** — grep it
- [ ] Excess precision rejected, not truncated
- [ ] `price_scale <= canonical_scale` asserted at load
- [ ] `Clear` emitted on every disconnect path, including panics and timeouts
- [ ] Gap → snapshot request → deltas dropped until it arrives
- [ ] Reconnect backoff capped; no tight loop on auth failure
- [ ] Cross-checked against a second provider's top of book
- [ ] Reject rate tracked and exposed
- [ ] Survives the sim adapter's gap / disconnect / stale / reject injections
