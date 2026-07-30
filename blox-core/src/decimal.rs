//! Decimal string <-> integer ticks. See `docs/ADAPTERS.md` §2.
//!
//! The single most likely place to reintroduce the bug integer ticks exist to
//! prevent. **Never** go through `f64`:
//!
//! ```text
//! let ticks = (s.parse::<f64>()? * 100_000.0) as i64;   // WRONG
//! ```
//!
//! `1.08501_f64 * 100000.0` is `108500.99999999999`, and `as i64` truncates
//! toward zero giving `108500` — one tick low, on some prices and not others,
//! depending on the bit pattern. It passes every test written by hand and
//! fails in production on the prices nobody thought to try.

#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum ParseErr {
    Empty,
    BadDigit,
    Overflow,
    /// More decimal places than the instrument's scale. Rejected rather than
    /// truncated — truncation hides a registry misconfiguration permanently,
    /// and hides it in the direction that makes prices look better.
    TooPrecise,
}

impl std::fmt::Display for ParseErr {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            ParseErr::Empty => "empty",
            ParseErr::BadDigit => "bad digit",
            ParseErr::Overflow => "overflow",
            ParseErr::TooPrecise => "more precision than the instrument scale",
        };
        f.write_str(s)
    }
}

impl std::error::Error for ParseErr {}

/// `"1.08501"` at scale 5 -> `108501`. Exact, no float ever exists.
///
/// Short fractions are zero-padded (`"1.0850"` @5 -> `108500`); long ones are
/// rejected unless the excess is all zeros.
pub fn parse_decimal(s: &str, scale: u32) -> Result<i64, ParseErr> {
    let s = s.trim();
    let (neg, s) = match s.as_bytes().first() {
        Some(b'-') => (true, &s[1..]),
        Some(b'+') => (false, &s[1..]),
        _ => (false, s),
    };

    let (int, frac) = s.split_once('.').unwrap_or((s, ""));
    if int.is_empty() && frac.is_empty() {
        return Err(ParseErr::Empty);
    }

    let mut v: i64 = 0;
    for b in int.bytes() {
        let d = (b as char).to_digit(10).ok_or(ParseErr::BadDigit)?;
        v = v
            .checked_mul(10)
            .and_then(|v| v.checked_add(d as i64))
            .ok_or(ParseErr::Overflow)?;
    }

    let fb = frac.as_bytes();
    for i in 0..scale as usize {
        let d = match fb.get(i) {
            Some(b) => (*b as char).to_digit(10).ok_or(ParseErr::BadDigit)?,
            None => 0, // pad short fractions
        };
        v = v
            .checked_mul(10)
            .and_then(|v| v.checked_add(d as i64))
            .ok_or(ParseErr::Overflow)?;
    }

    // Excess precision: only tolerated if it is all zeros.
    if fb.len() > scale as usize {
        for b in &fb[scale as usize..] {
            if !b.is_ascii_digit() {
                return Err(ParseErr::BadDigit);
            }
            if *b != b'0' {
                return Err(ParseErr::TooPrecise);
            }
        }
    }

    Ok(if neg { -v } else { v })
}

/// `108501` at scale 5 -> `"1.08501"`. Integer division only.
pub fn format_decimal(v: i64, scale: u32) -> String {
    if scale == 0 {
        return v.to_string();
    }
    let p = 10i64.pow(scale);
    let neg = v < 0;
    let a = v.unsigned_abs();
    let int = a / p as u64;
    let frac = a % p as u64;
    format!(
        "{}{}.{:0width$}",
        if neg { "-" } else { "" },
        int,
        frac,
        width = scale as usize
    )
}

/// Rescale a provider's price into canonical ticks.
///
/// Always a multiplication, because the canonical scale is the *finest* across
/// all providers (`DESIGN.md` D16). Dividing would silently discard the last
/// digit of your most precise provider — always in the direction that makes
/// their price look worse, so your best source would quietly lose every
/// routing decision.
/// A real `assert!`, not `debug_assert!`. The branch is two `u32`s and
/// perfectly predictable, while the failure it guards is silent, systematic
/// price corruption from one provider — the kind you discover months later as
/// "that LP mysteriously never wins any routing". Provider data is a trust
/// boundary; this check stays in release builds.
#[inline]
pub fn rescale(raw: i64, from_scale: u32, canonical_scale: u32) -> i64 {
    assert!(
        from_scale <= canonical_scale,
        "provider scale {from_scale} exceeds canonical {canonical_scale}; \
         canonical must be the finest scale across providers"
    );
    raw * 10i64.pow(canonical_scale - from_scale)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_exactly_where_the_float_path_does_not() {
        assert_eq!(parse_decimal("1.08501", 5).unwrap(), 108_501);
        // A real failing case, kept so nobody "simplifies" this back to a
        // float multiply. 1.00002 * 100000.0 == 100001.99999999999.
        assert_eq!(parse_decimal("1.00002", 5).unwrap(), 100_002);
        assert_eq!(("1.00002".parse::<f64>().unwrap() * 100_000.0) as i64, 100_001);
    }

    #[test]
    fn float_parsing_is_wrong_on_a_sixth_of_a_realistic_fx_range() {
        // Sweeps every 5dp price from 1.00000 to 1.20000 — an ordinary EURUSD
        // range. This is the whole argument for integer ticks, as a number.
        let mut float_failures = 0;
        for i in 100_000i64..120_000 {
            let s = format!("{}.{:05}", i / 100_000, i % 100_000);
            assert_eq!(parse_decimal(&s, 5).unwrap(), i, "exact parse of {s}");
            if (s.parse::<f64>().unwrap() * 100_000.0) as i64 != i {
                float_failures += 1;
            }
        }
        // ~3445 of 20000 at time of writing. The exact count is not the point;
        // that it is thousands, and silent, is.
        assert!(
            float_failures > 1000,
            "expected the float path to fail on a large fraction, got {float_failures}"
        );
    }

    #[test]
    fn padding_and_scales() {
        assert_eq!(parse_decimal("1.0850", 5).unwrap(), 108_500);
        assert_eq!(parse_decimal("1", 5).unwrap(), 100_000);
        assert_eq!(parse_decimal("0.5", 2).unwrap(), 50);
        assert_eq!(parse_decimal(".5", 2).unwrap(), 50);
        assert_eq!(parse_decimal("123", 0).unwrap(), 123);
    }

    #[test]
    fn signs() {
        assert_eq!(parse_decimal("-0.5", 2).unwrap(), -50);
        assert_eq!(parse_decimal("+1.25", 2).unwrap(), 125);
        assert_eq!(parse_decimal("-1.08501", 5).unwrap(), -108_501);
    }

    #[test]
    fn excess_precision_is_rejected_not_truncated() {
        assert_eq!(parse_decimal("1.085015", 5), Err(ParseErr::TooPrecise));
        // Trailing zeros beyond the scale are harmless.
        assert_eq!(parse_decimal("1.0850100", 5).unwrap(), 108_501);
    }

    #[test]
    fn bad_input() {
        assert_eq!(parse_decimal("", 5), Err(ParseErr::Empty));
        assert_eq!(parse_decimal(".", 5), Err(ParseErr::Empty));
        assert_eq!(parse_decimal("1.2x", 5), Err(ParseErr::BadDigit));
        assert_eq!(parse_decimal("abc", 5), Err(ParseErr::BadDigit));
        assert_eq!(parse_decimal("999999999999999999999", 5), Err(ParseErr::Overflow));
    }

    #[test]
    fn round_trips() {
        for (s, scale) in [
            ("1.08501", 5),
            ("0.00001", 5),
            ("-1.08501", 5),
            ("12345.67", 2),
            ("0.00", 2),
        ] {
            let v = parse_decimal(s, scale).unwrap();
            assert_eq!(format_decimal(v, scale), s, "round trip {s}");
        }
    }

    #[test]
    fn rescale_is_exact_multiplication() {
        // LP-B quotes 4dp, canonical is 5dp.
        assert_eq!(rescale(10_850, 4, 5), 108_500);
        // Same scale is a no-op.
        assert_eq!(rescale(108_501, 5, 5), 108_501);
    }

    #[test]
    #[should_panic(expected = "canonical must be the finest")]
    fn rescale_rejects_a_coarser_canonical_scale() {
        rescale(108_501, 5, 4);
    }
}
