//! Durations: parsing `90s`, `10m` or `1h30m`, and printing elapsed time.

use std::time::Duration;

/// Parses a duration such as `90s`, `10m`, `1h30m` or `0`.
pub fn parse_duration(s: &str) -> Result<Duration, String> {
    let bad = || format!("`{s}` is not a duration; write it like 90s, 10m or 1h30m");
    if s == "0" {
        return Ok(Duration::ZERO);
    }
    let mut total = 0u64;
    let mut digits = String::new();
    for c in s.chars() {
        if c.is_ascii_digit() {
            digits.push(c);
            continue;
        }
        let unit = match c {
            'h' => 3600,
            'm' => 60,
            's' => 1,
            _ => return Err(bad()),
        };
        let n: u64 = digits.parse().map_err(|_| bad())?;
        total = n
            .checked_mul(unit)
            .and_then(|n| total.checked_add(n))
            .ok_or_else(bad)?;
        digits.clear();
    }
    if !digits.is_empty() || s.is_empty() {
        return Err(bad());
    }
    Ok(Duration::from_secs(total))
}

/// Whole seconds in the same form `parse_duration` reads: `0s`, `45s`,
/// `6m12s` or `1h30m`.
pub fn elapsed(secs: u64) -> String {
    let (h, m, s) = (secs / 3600, secs / 60 % 60, secs % 60);
    let mut out = String::new();
    if h > 0 {
        out.push_str(&format!("{h}h"));
    }
    if m > 0 {
        out.push_str(&format!("{m}m"));
    }
    if s > 0 || out.is_empty() {
        out.push_str(&format!("{s}s"));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn durations() {
        assert_eq!(parse_duration("0"), Ok(Duration::ZERO));
        assert_eq!(parse_duration("90s"), Ok(Duration::from_secs(90)));
        assert_eq!(parse_duration("10m"), Ok(Duration::from_secs(600)));
        assert_eq!(parse_duration("1h30m"), Ok(Duration::from_secs(5400)));
        assert_eq!(parse_duration("0s"), Ok(Duration::ZERO));
        for bad in ["", "10", "m", "1x", "1h30", "-5s", "1.5m"] {
            assert!(parse_duration(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn elapsed_round_trips() {
        assert_eq!(elapsed(0), "0s");
        assert_eq!(elapsed(45), "45s");
        assert_eq!(elapsed(372), "6m12s");
        assert_eq!(elapsed(5400), "1h30m");
        assert_eq!(elapsed(3601), "1h1s");
        for secs in [0, 45, 372, 5400, 3601, 86_399] {
            assert_eq!(
                parse_duration(&elapsed(secs)),
                Ok(Duration::from_secs(secs))
            );
        }
    }
}
