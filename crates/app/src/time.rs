//! Local dates and relative ages used by Git views and recovery snapshots.

const DAY_MS: i64 = 86_400_000;

pub fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
}

/// Seconds east of UTC at `ms` (local time zone, daylight saving included).
pub fn local_offset(ms: i64) -> i64 {
    let secs = (ms / 1000) as libc::time_t;
    // SAFETY: `localtime_r` writes only into the `tm` we pass and reads `secs`; both live on
    // this stack frame for the whole call. A zeroed `tm` is a valid value of the C struct.
    unsafe {
        let mut tm: libc::tm = std::mem::zeroed();
        if libc::localtime_r(&secs, &mut tm).is_null() {
            return 0;
        }
        tm.tm_gmtoff
    }
}

/// Local calendar day number of `ms` (days since 1970-01-01 in local time).
fn local_day(ms: i64, offset: i64) -> i64 {
    (ms + offset * 1000).div_euclid(DAY_MS)
}

/// (year, month, day, weekday 0 = Monday, hour, minute) of a local day / time.
pub fn civil(ms: i64, offset: i64) -> (i64, u32, u32, u32, u32, u32) {
    let local = ms / 1000 + offset;
    let days = local.div_euclid(86_400);
    let secs = local.rem_euclid(86_400);
    // Howard Hinnant's days_from_civil inverse.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let month = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let year = yoe + era * 400 + i64::from(month <= 2);
    // 1970-01-01 was a Thursday (weekday 3 counting from Monday).
    let weekday = (days + 3).rem_euclid(7) as u32;
    (
        year,
        month,
        day,
        weekday,
        (secs / 3600) as u32,
        (secs % 3600 / 60) as u32,
    )
}

/// Relative age for Git commits and stashes.
pub fn relative_time(ms: i64, now: i64, offset: i64) -> String {
    let days = local_day(now, offset) - local_day(ms, offset);
    let (_, month, day, _, hour, minute) = civil(ms, offset);
    match days {
        i64::MIN..=0 => format!("今天 {hour:02}:{minute:02}"),
        1 => "昨天".into(),
        2..=6 => format!("{days} 天前"),
        _ => format!("{month}月{day}日"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_dates_and_git_ages_keep_their_existing_format() {
        const NOW: i64 = 1_790_937_000_000;
        const HOUR: i64 = 3_600_000;
        assert_eq!(civil(NOW, 0), (2026, 10, 2, 4, 10, 30));
        assert_eq!(civil(0, 0), (1970, 1, 1, 3, 0, 0));
        assert_eq!(civil(NOW, 8 * 3600).4, 18);
        assert_eq!(relative_time(NOW - HOUR, NOW, 0), "今天 09:30");
        assert_eq!(relative_time(NOW - 24 * HOUR, NOW, 0), "昨天");
        assert_eq!(relative_time(NOW - 3 * 24 * HOUR, NOW, 0), "3 天前");
        assert_eq!(relative_time(NOW - 30 * 24 * HOUR, NOW, 0), "9月2日");
        let _ = local_offset(NOW);
    }
}
