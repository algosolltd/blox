//! Deterministic workload generation for tests, benchmarks, and the replay
//! checker. Not used by the engine itself.
//!
//! The PRNG lives out here, never inside the engine — `docs/CORE.md` §9
//! forbids `rand` in the core. Seeding it in the caller means "reproduce the
//! exact sequence that broke it" is `--seed N`.

use crate::event::*;
use crate::types::*;

/// xorshift64*. Deterministic, no dependencies, good enough to shake out
/// linked-list bugs.
pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Self {
        // Never allow a zero state; xorshift would stick at zero forever.
        Rng(seed | 1)
    }

    pub fn next_u64(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    /// Uniform in `[0, n)`.
    pub fn below(&mut self, n: u64) -> u64 {
        if n == 0 {
            return 0;
        }
        self.next_u64() % n
    }

    pub fn range(&mut self, lo: i64, hi: i64) -> i64 {
        debug_assert!(hi > lo);
        lo + self.below((hi - lo) as u64) as i64
    }
}

/// Order-sensitive hash of a change stream. Two runs agreeing on this is the
/// determinism check; it is also what a golden-replay fixture stores, since
/// event logs compress well and change streams do not.
pub fn hash_changes(changes: &[Change]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325; // FNV-1a offset basis
    for c in changes {
        for b in format!("{c:?}").as_bytes() {
            h ^= *b as u64;
            h = h.wrapping_mul(0x1000_0000_01b3);
        }
        h ^= 0xff;
        h = h.wrapping_mul(0x1000_0000_01b3);
    }
    h
}

/// A mixed workload: several providers quoting several instruments, an
/// internal book taking orders, triggers, ticks, and disconnects.
///
/// Deliberately adversarial — duplicate ids, zero quantities, cancels of
/// things that were never placed — because the bugs that matter need a
/// *sequence*, not an input.
pub fn scripted_events(n: usize, seed: u64) -> Vec<Event> {
    let mut r = Rng::new(seed);
    let mut ev = Vec::with_capacity(n);
    let providers = [ProviderId(10), ProviderId(20), ProviderId(30)];
    let instruments = [InstrumentId(1), InstrumentId(2)];
    let mut now: u64 = 0;
    let mut next_id: OrderId = 1;
    let mut live: Vec<(InstrumentId, OrderId)> = Vec::new();
    let mut live_triggers: Vec<(InstrumentId, OrderId)> = Vec::new();
    let mut seeded = [[false; 2]; 3];

    // Every provider starts with a snapshot, so deltas always have a book.
    for (pi, p) in providers.iter().enumerate() {
        for (ii, i) in instruments.iter().enumerate() {
            ev.push(snapshot(&mut r, *p, *i));
            seeded[pi][ii] = true;
        }
    }

    while ev.len() < n {
        let roll = r.below(100);
        let pi = r.below(providers.len() as u64) as usize;
        let ii = r.below(instruments.len() as u64) as usize;
        let p = providers[pi];
        let inst = instruments[ii];

        match roll {
            // Provider deltas dominate, as they do in real feeds.
            0..=44 => {
                if !seeded[pi][ii] {
                    ev.push(snapshot(&mut r, p, inst));
                    seeded[pi][ii] = true;
                    continue;
                }
                ev.push(Event::Delta {
                    provider: p,
                    instrument: inst,
                    side: if r.below(2) == 0 { Side::Bid } else { Side::Ask },
                    price: r.range(9_900, 10_100),
                    qty: if r.below(8) == 0 { 0 } else { r.range(1, 50) },
                });
            }
            45..=51 => {
                ev.push(snapshot(&mut r, p, inst));
                seeded[pi][ii] = true;
            }
            // New internal orders, including deliberately invalid ones.
            52..=76 => {
                let id = if r.below(20) == 0 && !live.is_empty() {
                    live[r.below(live.len() as u64) as usize].1 // duplicate id
                } else {
                    let id = next_id;
                    next_id += 1;
                    id
                };
                let qty = match r.below(30) {
                    0 => 0,
                    1 => -5,
                    _ => r.range(1, 40),
                };
                let kind = match r.below(10) {
                    0 => OrderKind::Market,
                    1 => OrderKind::Ioc,
                    2 => OrderKind::Fok,
                    3 => OrderKind::PostOnly,
                    _ => OrderKind::Limit,
                };
                ev.push(Event::New {
                    instrument: inst,
                    id,
                    owner: r.below(4) as OwnerId,
                    side: if r.below(2) == 0 { Side::Bid } else { Side::Ask },
                    price: r.range(9_950, 10_050),
                    qty,
                    kind,
                });
                if qty > 0 && kind != OrderKind::Market {
                    live.push((inst, id));
                }
            }
            77..=84 => {
                // Cancel — sometimes something real, sometimes a phantom.
                if !live.is_empty() && r.below(4) > 0 {
                    let k = r.below(live.len() as u64) as usize;
                    let (i, id) = live.swap_remove(k);
                    ev.push(Event::Cancel { instrument: i, id });
                } else {
                    ev.push(Event::Cancel {
                        instrument: inst,
                        id: 900_000 + r.below(1_000),
                    });
                }
            }
            85..=88 => {
                if !live.is_empty() {
                    let k = r.below(live.len() as u64) as usize;
                    let (i, id) = live[k];
                    let qty = r.range(0, 60);
                    if qty == 0 {
                        live.swap_remove(k);
                    }
                    ev.push(Event::Amend {
                        instrument: i,
                        id,
                        qty,
                    });
                }
            }
            89..=93 => {
                let id = next_id;
                next_id += 1;
                let side = if r.below(2) == 0 { Side::Bid } else { Side::Ask };
                let kind = match r.below(3) {
                    0 => TriggerKind::Stop,
                    1 => TriggerKind::Limit,
                    _ => TriggerKind::StopLimit {
                        limit_price: r.range(9_900, 10_100),
                    },
                };
                ev.push(Event::PlaceTrigger {
                    trigger: Trigger::new(
                        id,
                        r.below(4) as OwnerId,
                        inst,
                        side,
                        r.range(9_800, 10_200),
                        r.range(1, 20),
                        kind,
                    ),
                });
                live_triggers.push((inst, id));
            }
            94..=95 => {
                if !live_triggers.is_empty() {
                    let k = r.below(live_triggers.len() as u64) as usize;
                    let (i, id) = live_triggers.swap_remove(k);
                    ev.push(Event::CancelTrigger { instrument: i, id });
                }
            }
            // Disconnects: the operation that must evict a provider atomically.
            96..=97 => {
                ev.push(Event::Clear { provider: p });
                seeded[pi] = [false; 2];
            }
            _ => {
                now += r.below(500) + 1;
                ev.push(Event::Tick { ts_nanos: now });
            }
        }
    }
    ev.truncate(n);
    ev
}

fn snapshot(r: &mut Rng, p: ProviderId, i: InstrumentId) -> Event {
    let mid = r.range(9_950, 10_050);
    let depth = r.below(6) as usize + 1;
    let mut bids = Vec::with_capacity(depth);
    let mut asks = Vec::with_capacity(depth);
    for d in 0..depth as i64 {
        bids.push((mid - 1 - d * (1 + r.range(0, 3)), r.range(1, 50)));
        asks.push((mid + 1 + d * (1 + r.range(0, 3)), r.range(1, 50)));
    }
    // The adapter contract requires sorted, deduplicated, positive levels.
    bids.sort_unstable_by_key(|a| std::cmp::Reverse(a.0));
    bids.dedup_by_key(|l| l.0);
    asks.sort_unstable_by_key(|a| a.0);
    asks.dedup_by_key(|l| l.0);
    Event::Snapshot {
        provider: p,
        instrument: i,
        bids,
        asks,
    }
}
