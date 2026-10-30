//! Durations (`90s`, `10m`, `1h30m`), printing elapsed time, points in time
//! given on the command line, and the UTC timestamps GitHub returns.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

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

/// Seconds since the Unix epoch for a UTC RFC 3339 timestamp such as
/// `2026-10-30T10:55:01Z`, the form GitHub's API uses. Fractional seconds are
/// ignored; other offsets are not accepted.
pub fn parse_utc(s: &str) -> Option<i64> {
    let s = s.strip_suffix('Z')?;
    let (date, time) = s.split_once('T')?;
    let time = time.split_once('.').map_or(time, |(whole, _)| whole);
    let mut date = date.splitn(3, '-').map(str::parse::<i64>);
    let (y, m, d) = (date.next()?.ok()?, date.next()?.ok()?, date.next()?.ok()?);
    let mut time = time.splitn(3, ':').map(str::parse::<i64>);
    let (hh, mm, ss) = (time.next()?.ok()?, time.next()?.ok()?, time.next()?.ok()?);
    if !(1..=12).contains(&m) || !(1..=31).contains(&d) || hh > 23 || mm > 59 || ss > 60 {
        return None;
    }
    Some(days_from_civil(y, m, d) * 86_400 + hh * 3600 + mm * 60 + ss)
}

/// Seconds since the Unix epoch for an RFC 3339 timestamp with a `Z` or
/// `±HH:MM` offset.
pub(crate) fn parse_rfc3339(s: &str) -> Option<i64> {
    if let Some(utc) = s.strip_suffix(['Z', 'z']) {
        return parse_utc(&format!("{utc}Z"));
    }
    let split = s.len().checked_sub(6)?;
    let (local, offset) = (s.get(..split)?, s.get(split..)?);
    let sign = match offset.as_bytes()[0] {
        b'+' => 1,
        b'-' => -1,
        _ => return None,
    };
    let (h, m) = offset[1..].split_once(':')?;
    let (h, m): (i64, i64) = (h.parse().ok()?, m.parse().ok()?);
    if h > 23 || m > 59 {
        return None;
    }
    Some(parse_utc(&format!("{local}Z"))? - sign * (h * 3600 + m * 60))
}

/// A point in time given on the command line: RFC 3339, or a duration
/// meaning that long before `now` (`2h` is two hours ago).
pub fn parse_time(s: &str, now: i64) -> Result<i64, String> {
    if let Some(t) = parse_rfc3339(s) {
        return Ok(t);
    }
    let ago = parse_duration(s).map_err(|_| {
        format!("`{s}` is not a time; write it as RFC 3339, a duration ago such as 2h, or @SHA")
    })?;
    Ok(now - i64::try_from(ago.as_secs()).unwrap_or(i64::MAX))
}

/// The current time in seconds since the Unix epoch.
pub fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs().try_into().unwrap_or(i64::MAX))
}

/// Formats seconds since the epoch as `2026-10-30T10:55:01Z`.
pub fn format_utc(secs: i64) -> String {
    let (days, rem) = (secs.div_euclid(86_400), secs.rem_euclid(86_400));
    let (y, m, d) = civil_from_days(days);
    format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z",
        rem / 3600,
        rem % 3600 / 60,
        rem % 60
    )
}

/// A compact, rounded-down length of time: `45s`, `12m`, `3h` or `2d`.
pub fn age(secs: i64) -> String {
    let secs = secs.max(0);
    match secs {
        0..60 => format!("{secs}s"),
        60..3600 => format!("{}m", secs / 60),
        3600..86_400 => format!("{}h", secs / 3600),
        _ => format!("{}d", secs / 86_400),
    }
}

/// Days from 1970-01-01 to a proleptic Gregorian date (Howard Hinnant's
/// `days_from_civil`).
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// The proleptic Gregorian date `days` after 1970-01-01 (Howard Hinnant's
/// `civil_from_days`).
fn civil_from_days(days: i64) -> (i64, i64, i64) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (yoe + era * 400 + i64::from(m <= 2), m, d)
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
    fn ages_round_down() {
        assert_eq!(age(-5), "0s");
        assert_eq!(age(59), "59s");
        assert_eq!(age(61), "1m");
        assert_eq!(age(7300), "2h");
        assert_eq!(age(200_000), "2d");
    }

    #[test]
    fn command_line_times() {
        let t = 1_793_357_701;
        assert_eq!(parse_time("2026-10-30T10:55:01Z", 0), Ok(t));
        assert_eq!(parse_time("2026-10-30T12:55:01+02:00", 0), Ok(t));
        assert_eq!(parse_time("2026-10-30T05:55:01.5-05:00", 0), Ok(t));
        assert_eq!(parse_time("2h", t), Ok(t - 7200));
        for bad in ["yesterday", "2026-10-30", "2026-10-30T10:55:01+2:00", "é"] {
            assert!(parse_time(bad, t).is_err(), "{bad}");
        }
    }

    #[test]
    fn utc_timestamps() {
        assert_eq!(parse_utc("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(parse_utc("2000-03-01T00:00:00Z"), Some(951_868_800));
        assert_eq!(parse_utc("2026-10-30T10:55:01Z"), Some(1_793_357_701));
        assert_eq!(parse_utc("2026-10-30T10:55:01.123Z"), Some(1_793_357_701));
        for t in [0, 951_868_800, 1_793_357_701, -86_401] {
            assert_eq!(parse_utc(&format_utc(t)), Some(t), "{t}");
        }
        assert_eq!(format_utc(1_793_357_701), "2026-10-30T10:55:01Z");
        for bad in [
            "",
            "2026-10-30",
            "2026-10-30T10:55:01",
            "2026-13-01T00:00:00Z",
        ] {
            assert_eq!(parse_utc(bad), None, "{bad}");
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
