//! Provider registry + shared adapter helpers, mirroring `src/providers/index.ts`
//! and `src/providers/common.ts`.

pub mod agy;
pub mod usage_fetch_failure;

use crate::lib::source_attempts;
use crate::lib::time;
use crate::types::{
    ProviderAdapter, ProviderId, ProviderQuota, ProviderSource, ProviderStatus, QuotaWindow,
    SourceAttempt,
};

/// Every registered adapter, in `PROVIDER_IDS` declaration order. agy is the first
/// port (epic #8); the credential-bearing providers land in follow-on PRs.
pub static ADAPTERS: [&'static dyn ProviderAdapter; 1] = [agy::AGY];

pub fn adapter_for(id: ProviderId) -> Option<&'static dyn ProviderAdapter> {
    ADAPTERS.iter().find(|a| a.id() == id).copied()
}

pub fn with_remaining(mut window: QuotaWindow) -> QuotaWindow {
    window.percent_remaining = time::percent_remaining(window.percent_used);
    window
}

pub struct SuccessArgs {
    pub provider: ProviderId,
    pub label: String,
    pub source: ProviderSource,
    pub plan: Option<String>,
    pub account: Option<crate::types::Account>,
    pub windows: Vec<QuotaWindow>,
    pub credits: Option<crate::types::Credits>,
    pub refreshed_at: String,
    pub sources_tried: Vec<String>,
    pub attempts: Option<Vec<SourceAttempt>>,
}

pub fn success_provider(args: SuccessArgs) -> ProviderQuota {
    ProviderQuota {
        provider: args.provider,
        label: Some(args.label),
        source: Some(args.source),
        plan: args.plan,
        account: args.account,
        windows: args.windows,
        quota_semantics: None,
        credits: args.credits,
        state: crate::types::ProviderState {
            status: ProviderStatus::Fresh,
            stale: false,
            refreshed_at: Some(args.refreshed_at),
            error: None,
            retry_after: None,
            auth_status: None,
            reason: None,
            remedy_command: None,
            untrusted_window_ids: None,
            degraded_sources: None,
            sources_tried: Some(args.sources_tried),
        },
        attempts: args.attempts,
    }
}

pub fn failed_provider(
    provider: ProviderId,
    label: &str,
    status: ProviderStatus,
    error: &str,
    sources_tried: Vec<String>,
    source: Option<ProviderSource>,
    retry_after: Option<String>,
    attempts: Option<Vec<SourceAttempt>>,
) -> ProviderQuota {
    ProviderQuota {
        provider,
        label: Some(label.to_string()),
        source: Some(source.unwrap_or(ProviderSource::Unavailable)),
        plan: None,
        account: None,
        windows: Vec::new(),
        quota_semantics: None,
        credits: None,
        state: crate::types::ProviderState {
            status,
            stale: false,
            refreshed_at: None,
            error: Some(error.to_string()),
            retry_after,
            auth_status: None,
            reason: None,
            remedy_command: None,
            untrusted_window_ids: None,
            degraded_sources: None,
            sources_tried: Some(sources_tried),
        },
        attempts,
    }
}

pub fn stale_from_cache(
    cached: ProviderQuota,
    error: &str,
    sources_tried: Vec<String>,
    attempts: Vec<SourceAttempt>,
) -> ProviderQuota {
    let mut merged: Vec<String> = sources_tried;
    if !merged.iter().any(|s| s == "cache") {
        merged.push("cache".to_string());
    }
    let mut state = cached.state.clone();
    state.status = ProviderStatus::Stale;
    state.stale = true;
    state.error = Some(error.to_string());
    state.sources_tried = Some(merged);
    ProviderQuota {
        provider: cached.provider,
        label: cached.label,
        source: Some(ProviderSource::Cache),
        plan: cached.plan,
        account: cached.account,
        windows: cached.windows,
        quota_semantics: cached.quota_semantics,
        credits: cached.credits,
        state,
        attempts: Some(attempts),
    }
}

pub fn status_from_error(error: &str) -> ProviderStatus {
    let lower = error.to_lowercase();
    if error == "keychain_prompt_required"
        || error == "credentials_expired"
        || lower.contains("sign-in")
        || lower.contains("required")
        || lower.contains("reauth")
        || lower.contains("access token expired")
    {
        return ProviderStatus::AuthRequired;
    }
    if lower.contains("rate limit") || lower.contains("ratelimit") || lower.contains("rate-limit") {
        return ProviderStatus::RateLimited;
    }
    ProviderStatus::Error
}

/// Attempt order, deduplicated.
pub fn source_names(attempts: &[SourceAttempt]) -> Vec<String> {
    let mut seen: Vec<String> = Vec::new();
    for attempt in attempts {
        if !seen.contains(&attempt.source) {
            seen.push(attempt.source.clone());
        }
    }
    seen
}

/// Re-exported so interpretation/pace can classify degraded sources on fresh readings.
pub fn degraded_sources(attempts: Option<&[SourceAttempt]>) -> Vec<crate::types::DegradedSource> {
    source_attempts::degraded_sources(attempts)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::AttemptStatus;

    #[test]
    fn status_from_error_buckets() {
        assert_eq!(status_from_error("rate limited"), ProviderStatus::RateLimited);
        assert_eq!(status_from_error("sign-in required"), ProviderStatus::AuthRequired);
        assert_eq!(status_from_error("credentials_expired"), ProviderStatus::AuthRequired);
        assert_eq!(status_from_error("boom"), ProviderStatus::Error);
    }

    #[test]
    fn source_names_dedupes() {
        let attempts = vec![
            SourceAttempt {
                source: "a".into(),
                status: AttemptStatus::Failed,
                error: None,
                credential_present: None,
                degraded: None,
            },
            SourceAttempt {
                source: "a".into(),
                status: AttemptStatus::Success,
                error: None,
                credential_present: None,
                degraded: None,
            },
            SourceAttempt {
                source: "b".into(),
                status: AttemptStatus::Success,
                error: None,
                credential_present: None,
                degraded: None,
            },
        ];
        assert_eq!(source_names(&attempts), vec!["a".to_string(), "b".to_string()]);
    }
}
