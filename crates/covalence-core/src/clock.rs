//! Wall-clock helpers.

use std::time::{SystemTime, UNIX_EPOCH};

/// Unix epoch milliseconds now.
pub fn epoch_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64)
}

/// RFC 3339 in UTC with microseconds: `2026-10-18T12:34:56.789012Z`.
pub fn rfc3339_micros(time: SystemTime) -> String {
    let micros = time
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_micros() as i64);
    let secs = micros.div_euclid(1_000_000);
    let frac = micros.rem_euclid(1_000_000);
    let days = secs.div_euclid(86_400);
    let tod = secs.rem_euclid(86_400);
    let (y, m, d) = civil_from_days(days);
    format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}.{frac:06}Z",
        tod / 3600,
        tod / 60 % 60,
        tod % 60
    )
}

/// Days since 1970-01-01 to a proleptic Gregorian date (Howard Hinnant's algorithm).
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
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
    use std::time::Duration;

    use super::*;

    #[test]
    fn formats_rfc3339_with_micros() {
        let t = UNIX_EPOCH + Duration::from_micros(1_792_326_896_123_456);
        assert_eq!(rfc3339_micros(t), "2026-10-18T12:34:56.123456Z");
        assert_eq!(rfc3339_micros(UNIX_EPOCH), "1970-01-01T00:00:00.000000Z");
        let leap = UNIX_EPOCH + Duration::from_secs(951_782_400);
        assert_eq!(rfc3339_micros(leap), "2000-02-29T00:00:00.000000Z");
    }
}
