//! Replay a seeded workload and print a hash of the resulting change stream.
//!
//! Exists so determinism can be checked *across processes*. `HashMap`'s
//! `RandomState` is seeded per process, so an in-process rerun cannot catch a
//! bug where hash iteration order leaks into the output — two runs in one
//! process would share the same seed and agree while still being wrong.
//!
//! ```text
//! replay_hash <seed> <event-count>
//! ```

use blox_core::testkit::{hash_changes, scripted_events};
use blox_core::Engine;

fn main() {
    let mut args = std::env::args().skip(1);
    let seed: u64 = args
        .next()
        .and_then(|s| s.parse().ok())
        .unwrap_or(0xDEAD_BEEF);
    let count: usize = args.next().and_then(|s| s.parse().ok()).unwrap_or(20_000);

    let events = scripted_events(count, seed);
    let mut engine = Engine::new().with_max_age_ns(5_000);
    let mut out = Vec::new();
    engine.apply_batch(&events, &mut out);

    if let Err(msg) = engine.check() {
        eprintln!("INVARIANT BREACH: {msg}");
        std::process::exit(2);
    }

    println!("{:016x} {} {}", hash_changes(&out), out.len(), events.len());
}
