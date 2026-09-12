//! Time helpers mirroring `src/lib/time.ts`.

use chrono::{DateTime, SecondsFormat, Utc};

/// Current time as ISO-8601 with milliseconds and `Z` (matches `new Date().toISOString()`).
pub fn now_iso() -> String {
    Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true)
}

pub fn clamp_percent(value: f64) -> f64 {
    if !value.is_finite() {
        return 0.0;
    }
    value.round().clamp(0.0, 100.0)
}

pub fn percent_remaining(percent_used: Option<f64>) -> Option<f64> {
    percent_used.map(|used| clamp_percent(100.0 - used))
}

/// `parseEpochOrIso`: an epoch-seconds number or an ISO string; a non-date string passes
/// through unchanged (matching the Node behavior).
pub fn parse_epoch_or_iso(value: &serde_json::Value) -> Option<String> {
    match value {
        serde_json::Value::Number(n) => {
            let secs = n.as_f64()?;
            if !secs.is_finite() {
                return None;
            }
            DateTime::<Utc>::from_timestamp(secs as i64, 0)
                .map(|d| d.to_rfc3339_opts(SecondsFormat::Millis, true))
        }
        serde_json::Value::String(s) if !s.trim().is_empty() => {
            match DateTime::parse_from_rfc3339(s) {
                Ok(d) => Some(d.with_timezone(&Utc).to_rfc3339_opts(SecondsFormat::Millis, true)),
                Err(_) => Some(s.clone()),
            }
        }
        _ => None,
    }
}

/// `retryAfterToIso`: a number of seconds or an ISO date, relative to `now`.
pub fn retry_after_to_iso(value: Option<&str>, now: i64) -> Option<String> {
    let raw = value?.trim();
    if raw.is_empty() {
        return None;
    }
    if let Ok(seconds) = raw.parse::<f64>() {
        if seconds.is_finite() && seconds >= 0.0 {
            return DateTime::<Utc>::from_timestamp(now + seconds as i64, 0)
                .map(|d| d.to_rfc3339_opts(SecondsFormat::Millis, true));
        }
    }
    match DateTime::parse_from_rfc3339(raw) {
        Ok(d) => Some(d.with_timezone(&Utc).to_rfc3339_opts(SecondsFormat::Millis, true)),
        Err(_) => None,
    }
}
