//! UTC timestamps for provenance records, without a date crate.

/// The current time as an ISO 8601 UTC timestamp: `2026-10-31T14:05:09Z`.
pub fn now_iso8601() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or_default();
    iso8601_utc(secs)
}

/// Format seconds since the Unix epoch as an ISO 8601 UTC timestamp.
pub fn iso8601_utc(unix_secs: u64) -> String {
    let days = (unix_secs / 86_400) as i64;
    let rem = unix_secs % 86_400;
    let (y, m, d) = civil_from_days(days);
    format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z",
        rem / 3600,
        (rem % 3600) / 60,
        rem % 60
    )
}

/// Days since 1970-01-01 to a (year, month, day) civil date
/// (Howard Hinnant's algorithm, proleptic Gregorian).
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let y = yoe + era * 400 + i64::from(m <= 2);
    (y, m, d)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_instants() {
        assert_eq!(iso8601_utc(0), "1970-01-01T00:00:00Z");
        // A leap day and the last second of a century leap year.
        assert_eq!(iso8601_utc(951_782_400), "2000-02-29T00:00:00Z");
        assert_eq!(iso8601_utc(978_307_199), "2000-12-31T23:59:59Z");
        assert_eq!(iso8601_utc(1_793_480_737), "2026-10-31T21:05:37Z");
    }

    #[test]
    fn now_is_shaped_like_iso8601() {
        let s = now_iso8601();
        assert_eq!(s.len(), 20, "{s}");
        assert!(s.ends_with('Z') && s.as_bytes()[10] == b'T', "{s}");
    }
}
