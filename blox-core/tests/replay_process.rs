//! Cross-process determinism.
//!
//! The in-process checks in `determinism.rs` cannot catch a bug where
//! `HashMap` iteration order reaches the output: both runs share one
//! process-wide `RandomState` seed, so they agree with each other while both
//! being wrong. Running in separate processes gives different seeds, so any
//! address- or hash-order dependence shows up as a differing hash.

use std::process::Command;

fn run(seed: u64, count: usize) -> String {
    let out = Command::new(env!("CARGO_BIN_EXE_replay_hash"))
        .arg(seed.to_string())
        .arg(count.to_string())
        .output()
        .expect("failed to run replay_hash");
    assert!(
        out.status.success(),
        "replay_hash failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).unwrap().trim().to_string()
}

#[test]
fn separate_processes_produce_identical_output() {
    for seed in [0xDEAD_BEEFu64, 0x1234, 0xFFFF_FFFF, 7] {
        let first = run(seed, 20_000);
        for attempt in 1..4 {
            let again = run(seed, 20_000);
            assert_eq!(
                first, again,
                "seed {seed:#x} diverged across processes on attempt {attempt}. \
                 Something non-deterministic reached the output — most likely \
                 HashMap iteration order. See docs/CORE.md §9."
            );
        }
    }
}

#[test]
fn different_seeds_produce_different_output() {
    // Guards against the checker trivially passing because it hashes nothing.
    let a = run(1, 5_000);
    let b = run(2, 5_000);
    assert_ne!(a, b, "different workloads must not hash the same");
    assert!(!a.starts_with("0000000000000000"), "hash looks empty: {a}");
}
