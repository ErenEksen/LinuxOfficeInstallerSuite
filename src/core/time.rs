//! Minimal UTC timestamp formatting, so log names and log lines need no date crate.

use std::time::{SystemTime, UNIX_EPOCH};

/// (year, month, day, hour, minute, second) in UTC.
pub fn utc_parts(t: SystemTime) -> (i64, u32, u32, u32, u32, u32) {
    let secs = t.duration_since(UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0);
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    // Howard Hinnant's civil_from_days.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let y = yoe + era * 400 + if m <= 2 { 1 } else { 0 };
    (y, m, d, (rem / 3600) as u32, (rem % 3600 / 60) as u32, (rem % 60) as u32)
}

/// `2026-10-19T14-05-03Z`, safe for file names.
pub fn file_stamp(t: SystemTime) -> String {
    let (y, mo, d, h, mi, s) = utc_parts(t);
    format!("{y:04}-{mo:02}-{d:02}T{h:02}-{mi:02}-{s:02}Z")
}

/// `14:05:03`, for log line prefixes.
pub fn clock(t: SystemTime) -> String {
    let (_, _, _, h, mi, s) = utc_parts(t);
    format!("{h:02}:{mi:02}:{s:02}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn formats_known_instants() {
        assert_eq!(file_stamp(UNIX_EPOCH), "1970-01-01T00-00-00Z");
        // 2026-10-19 14:05:03 UTC
        let t = UNIX_EPOCH + Duration::from_secs(1_792_418_703);
        assert_eq!(file_stamp(t), "2026-10-19T14-05-03Z");
        assert_eq!(clock(t), "14:05:03");
        // Leap day.
        let t = UNIX_EPOCH + Duration::from_secs(1_709_164_800);
        assert_eq!(file_stamp(t), "2024-02-29T00-00-00Z");
    }
}
