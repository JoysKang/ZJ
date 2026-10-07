//! Codex account quota: bounded local rollout reads for passive updates, plus parsing of
//! the live app-server response. The agent client owns that short-lived connection;
//! this module never reads credentials or makes network requests.

use serde_json::Value;
use std::{
    fs,
    io::{Read, Seek, SeekFrom},
    path::{Path, PathBuf},
    time::Duration,
};

pub const REFRESH_INTERVAL: Duration = Duration::from_secs(10 * 60);

/// Logs looked at, newest first, and how much of the end of each is read.
const LOGS: usize = 3;
const TAIL: u64 = 1024 * 1024;

#[derive(Clone, Debug, PartialEq)]
pub struct Quota {
    /// e.g. "pro", when Codex reports it.
    pub plan: Option<String>,
    pub windows: Vec<Window>,
    /// Remaining credits ("无限" when unlimited).
    pub credits: Option<String>,
    /// When Codex wrote it (Unix ms).
    pub updated_ms: i64,
    pub live: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Window {
    pub minutes: u64,
    pub used_percent: f64,
    /// Unix seconds.
    pub resets_at: Option<i64>,
}

impl Window {
    pub fn left_percent(&self) -> f64 {
        (100.0 - self.used_percent).clamp(0.0, 100.0)
    }
}

impl Quota {
    /// The tightest window's share left, for the composer's button.
    pub fn left_percent(&self) -> Option<f64> {
        self.windows
            .iter()
            .map(Window::left_percent)
            .reduce(f64::min)
    }
}

pub fn codex_home() -> Option<PathBuf> {
    std::env::var_os("CODEX_HOME")
        .filter(|home| !home.is_empty())
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".codex")))
}

/// The newest rate limits in the newest logs; `None` when Codex has written none (not used
/// yet, or an API key, which has no plan limits).
pub fn read_codex(home: &Path) -> Option<Quota> {
    newest_logs(&home.join("sessions"))
        .iter()
        .find_map(|log| tail(log).and_then(|text| text.lines().rev().find_map(parse)))
}

/// `sessions/YYYY/MM/DD/*.jsonl`, newest first (the names sort by date and time).
fn newest_logs(sessions: &Path) -> Vec<PathBuf> {
    let sorted = |dir: &Path| -> Vec<PathBuf> {
        let mut entries: Vec<PathBuf> = fs::read_dir(dir)
            .map(|entries| entries.flatten().map(|e| e.path()).collect())
            .unwrap_or_default();
        entries.sort_unstable_by(|a, b| b.cmp(a));
        entries
    };
    let mut logs = Vec::new();
    for year in sorted(sessions) {
        for month in sorted(&year) {
            for day in sorted(&month) {
                logs.extend(
                    sorted(&day)
                        .into_iter()
                        .filter(|p| p.extension().is_some_and(|e| e == "jsonl")),
                );
                if logs.len() >= LOGS {
                    logs.truncate(LOGS);
                    return logs;
                }
            }
        }
    }
    logs
}

fn tail(path: &Path) -> Option<String> {
    let mut file = fs::File::open(path).ok()?;
    let len = file.metadata().ok()?.len();
    file.seek(SeekFrom::Start(len.saturating_sub(TAIL))).ok()?;
    let mut bytes = Vec::new();
    file.take(TAIL).read_to_end(&mut bytes).ok()?;
    // The first line may start mid-character; it is skipped anyway (no `rate_limits` cut short
    // would parse).
    Some(String::from_utf8_lossy(&bytes).into_owned())
}

fn parse(line: &str) -> Option<Quota> {
    if !line.contains("\"rate_limits\"") {
        return None;
    }
    let event: Value = serde_json::from_str(line).ok()?;
    let limits = event.pointer("/payload/rate_limits")?;
    let windows: Vec<Window> = ["primary", "secondary"]
        .iter()
        .filter_map(|key| {
            let window = limits.get(key)?;
            Some(Window {
                minutes: window.get("window_minutes")?.as_u64()?,
                used_percent: window.get("used_percent")?.as_f64()?,
                resets_at: window.get("resets_at").and_then(Value::as_i64),
            })
        })
        .collect();
    if windows.is_empty() {
        return None;
    }
    let credits = limits.get("credits").and_then(|credits| {
        if credits.get("unlimited").and_then(Value::as_bool) == Some(true) {
            return Some("无限".to_string());
        }
        if credits.get("has_credits").and_then(Value::as_bool) != Some(true) {
            return None;
        }
        let balance: f64 = credits.get("balance")?.as_str()?.parse().ok()?;
        Some(format!("{balance:.2}"))
    });
    Some(Quota {
        plan: limits
            .get("plan_type")
            .and_then(Value::as_str)
            .map(str::to_string),
        windows,
        credits,
        updated_ms: event
            .get("timestamp")
            .and_then(Value::as_str)
            .and_then(utc_ms)
            .unwrap_or(0),
        live: false,
    })
}

/// The official app-server response, queried now rather than read from a rollout log.
pub fn from_live(result: &Value, now_ms: i64) -> Result<Quota, String> {
    let limits = result
        .pointer("/rateLimitsByLimitId/codex")
        .or_else(|| result.get("rateLimits"))
        .ok_or("Codex 未返回账户额度")?;
    let windows: Vec<Window> = ["primary", "secondary"]
        .iter()
        .filter_map(|key| {
            let w = limits.get(key)?;
            let used = w.get("usedPercent")?.as_f64()?;
            let minutes = w.get("windowDurationMins")?.as_u64()?;
            (used.is_finite() && minutes > 0).then(|| Window {
                minutes,
                used_percent: used,
                resets_at: w.get("resetsAt").and_then(Value::as_i64),
            })
        })
        .collect();
    if windows.is_empty() {
        return Err("当前 Codex 账户没有可查询的套餐额度".into());
    }
    let credits = limits.get("credits").and_then(|c| {
        if c.get("unlimited").and_then(Value::as_bool) == Some(true) {
            return Some("无限".into());
        }
        if c.get("hasCredits").and_then(Value::as_bool) != Some(true) {
            return None;
        }
        let balance = c.get("balance")?;
        let n = balance
            .as_f64()
            .or_else(|| balance.as_str()?.parse().ok())?;
        Some(format!("{n:.2}"))
    });
    Ok(Quota {
        windows,
        credits,
        plan: limits
            .get("planType")
            .and_then(Value::as_str)
            .map(str::to_string),
        updated_ms: now_ms,
        live: true,
    })
}

/// `2026-10-06T04:13:12.390Z` → Unix ms.
fn utc_ms(stamp: &str) -> Option<i64> {
    let num = |range: std::ops::Range<usize>| stamp.get(range)?.parse::<i64>().ok();
    let (year, month, day) = (num(0..4)?, num(5..7)?, num(8..10)?);
    let (hour, minute, second) = (num(11..13)?, num(14..16)?, num(17..19)?);
    let millis = stamp
        .get(19..)
        .and_then(|rest| rest.strip_prefix('.'))
        .map_or(0, |rest| {
            let digits: String = rest
                .chars()
                .take_while(char::is_ascii_digit)
                .take(3)
                .collect();
            format!("{digits:0<3}").parse().unwrap_or(0)
        });
    // Howard Hinnant's days_from_civil.
    let y = if month <= 2 { year - 1 } else { year };
    let era = y.div_euclid(400);
    let yoe = y.rem_euclid(400);
    let mp = (month + 9) % 12;
    let doy = (153 * mp + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    Some(((days * 24 + hour) * 60 + minute) * 60_000 + second * 1000 + millis)
}

/// 300 → "5 小时", 10080 → "每周".
pub fn window_label(minutes: u64) -> String {
    match minutes {
        10_080 => "每周".into(),
        1_440 => "每天".into(),
        m if m % 1_440 == 0 => format!("{} 天", m / 1_440),
        m if m % 60 == 0 => format!("{} 小时", m / 60),
        m => format!("{m} 分钟"),
    }
}

/// Time left until a reset: "3d 20h", "4h 12m", "35m".
pub fn countdown(secs: i64) -> String {
    let minutes = secs.max(0) / 60;
    let (days, hours, minutes) = (minutes / 1_440, minutes / 60 % 24, minutes % 60);
    if days > 0 {
        format!("{days}d {hours}h")
    } else if hours > 0 {
        format!("{hours}h {minutes}m")
    } else {
        format!("{minutes}m")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const LINE: &str = r#"{"timestamp":"2026-10-06T04:13:12.390Z","type":"event_msg","payload":{"type":"token_count","rate_limits":{"primary":{"used_percent":2.0,"window_minutes":10080,"resets_at":1791594020},"secondary":null,"credits":{"has_credits":true,"unlimited":false,"balance":"62428.4555"},"plan_type":"pro"}}}"#;

    #[test]
    fn the_newest_rate_limits_win() {
        let home = std::env::temp_dir().join(format!("zj-quota-{}", std::process::id()));
        let day = home.join("sessions/2026/10/06");
        fs::create_dir_all(&day).unwrap();
        let older = LINE.replace("2.0", "50.0");
        // The newest log's last `rate_limits` line counts; one without limits is passed over.
        fs::write(day.join("rollout-2026-10-06T09-00-00-a.jsonl"), &older).unwrap();
        fs::write(
            day.join("rollout-2026-10-06T12-00-00-b.jsonl"),
            format!("{older}\n{LINE}\n{{\"payload\":{{\"rate_limits\":null}}}}\n"),
        )
        .unwrap();
        let quota = read_codex(&home).unwrap();
        assert_eq!(quota.plan.as_deref(), Some("pro"));
        assert_eq!(quota.windows.len(), 1);
        assert_eq!(quota.left_percent(), Some(98.0));
        assert_eq!(quota.windows[0].resets_at, Some(1_791_594_020));
        assert_eq!(quota.credits.as_deref(), Some("62428.46"));
        assert_eq!(quota.updated_ms, 1_791_259_992_390);
        assert_eq!(read_codex(&home.join("missing")), None);
        let _ = fs::remove_dir_all(home);
    }

    #[test]
    fn live_limits_use_the_codex_bucket_and_the_query_time() {
        let result = serde_json::json!({"rateLimits":{"primary":{"usedPercent":99,"windowDurationMins":300}},"rateLimitsByLimitId":{"codex":{"planType":"pro","primary":{"usedPercent":12,"windowDurationMins":300,"resetsAt":100},"secondary":{"usedPercent":40,"windowDurationMins":10080},"credits":{"hasCredits":true,"unlimited":false,"balance":"10.5"}}}});
        let quota = from_live(&result, 12345).unwrap();
        assert!(quota.live);
        assert_eq!(quota.updated_ms, 12345);
        assert_eq!(quota.left_percent(), Some(60.0));
        assert_eq!(quota.credits.as_deref(), Some("10.50"));
        assert!(from_live(&serde_json::json!({"rateLimits":{"primary":null}}), 9).is_err());
    }

    #[test]
    fn labels_and_countdowns() {
        assert_eq!(window_label(300), "5 小时");
        assert_eq!(window_label(10_080), "每周");
        assert_eq!(window_label(43_200), "30 天");
        assert_eq!(countdown(3 * 86_400 + 20 * 3_600 + 59), "3d 20h");
        assert_eq!(countdown(4 * 3_600 + 12 * 60), "4h 12m");
        assert_eq!(countdown(-5), "0m");
        assert_eq!(utc_ms("1970-01-02T00:00:01Z"), Some(86_401_000));
    }
}
