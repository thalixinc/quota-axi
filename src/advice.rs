//! Situational advice annotation mirroring `src/advice.ts`.

use crate::types::{AttemptStatus, ProviderQuota, ProviderStateReason, QuotaAxiResponse};

pub const KEYCHAIN_ACCESS_REASON: &str = "keychain_access_required";
pub const KEYCHAIN_ACCESS_REMEDY_COMMAND: &str = "quota-axi --allow-keychain-prompt";
pub const CREDENTIALS_EXPIRED_REASON: &str = "credentials_expired";
pub const GROK_TOKEN_REFRESH_REMEDY_COMMAND: &str = "grok";

pub fn annotate_quota_advice(response: QuotaAxiResponse) -> QuotaAxiResponse {
    let providers: Vec<ProviderQuota> = response
        .providers
        .into_iter()
        .map(annotate_provider_advice)
        .collect();
    let help: Vec<String> = providers
        .iter()
        .flat_map(|p| provider_help_lines(p))
        .collect();
    QuotaAxiResponse {
        generated_at: response.generated_at,
        schema_version: 5,
        providers,
        help: if help.is_empty() { None } else { Some(help) },
    }
}

/// Situational advice stays first; only the tier hint repeats on every invocation.
pub fn quota_help_lines(response: &QuotaAxiResponse) -> Vec<String> {
    let mut lines: Vec<String> = response.help.clone().unwrap_or_default();
    lines.push("Run `quota-axi --full` for windows, pace, reserve, and account evidence".into());
    lines
}

fn annotate_provider_advice(mut provider: ProviderQuota) -> ProviderQuota {
    if needs_keychain_access_advice(&provider) {
        provider.state.reason = Some(ProviderStateReason::KeychainAccessRequired);
        provider.state.remedy_command = Some(KEYCHAIN_ACCESS_REMEDY_COMMAND.into());
    } else if needs_grok_token_refresh_advice(&provider) {
        provider.state.reason = Some(ProviderStateReason::CredentialsExpired);
        provider.state.remedy_command = Some(GROK_TOKEN_REFRESH_REMEDY_COMMAND.into());
    }
    provider
}

fn needs_keychain_access_advice(provider: &ProviderQuota) -> bool {
    let attempts = provider.attempts.as_deref().unwrap_or(&[]);
    provider.state.status != crate::types::ProviderStatus::Fresh
        && !attempts.iter().any(|a| a.status == AttemptStatus::Success)
        && attempts.iter().any(is_blocked_credential_attempt)
        && attempts.iter().any(is_prompt_blocked_keychain_attempt)
}

fn needs_grok_token_refresh_advice(provider: &ProviderQuota) -> bool {
    provider.provider == crate::types::ProviderId::Grok
        && provider.state.status != crate::types::ProviderStatus::Fresh
        && grok_cli_refresh_needed(provider)
}

/// Wired with the grok provider port (epic #8); no grok adapter exists yet.
fn grok_cli_refresh_needed(_provider: &ProviderQuota) -> bool {
    false
}

fn is_blocked_credential_attempt(attempt: &crate::types::SourceAttempt) -> bool {
    if is_keychain_source(&attempt.source) {
        return false;
    }
    if attempt.status == AttemptStatus::Skipped {
        return true;
    }
    attempt.status == AttemptStatus::Failed && is_definitive_credential_rejection(attempt.error.as_deref())
}

fn is_definitive_credential_rejection(error: Option<&str>) -> bool {
    let Some(error) = error else { return false };
    let lower = error.to_lowercase();
    let credentials = ["credentials_missing", "credentials_invalid", "credentials_expired"]
        .iter()
        .any(|p| lower == *p);
    credentials
        || lower.contains("sign-in required")
        || lower.contains("unauthorized")
        || lower.contains("forbidden")
        || lower.contains("401")
        || lower.contains("403")
}

fn is_keychain_source(source: &str) -> bool {
    source == "keychain" || source.ends_with("-keychain")
}

fn is_prompt_blocked_keychain_attempt(attempt: &crate::types::SourceAttempt) -> bool {
    is_keychain_source(&attempt.source)
        && attempt.status == AttemptStatus::Skipped
        && attempt.error.as_deref() == Some("keychain_prompt_required")
        && attempt.credential_present == Some(true)
}

fn provider_help_lines(provider: &ProviderQuota) -> Vec<String> {
    if provider.state.reason == Some(ProviderStateReason::KeychainAccessRequired)
        && provider.state.remedy_command.as_deref() == Some(KEYCHAIN_ACCESS_REMEDY_COMMAND)
    {
        return vec![format!(
            "Tell your user: run `{KEYCHAIN_ACCESS_REMEDY_COMMAND}` once and approve Keychain access (\"Always Allow\") so quota-axi can read {}'s live quota.",
            provider.provider
        )];
    }
    if provider.state.reason == Some(ProviderStateReason::CredentialsExpired)
        && provider.state.remedy_command.as_deref() == Some(GROK_TOKEN_REFRESH_REMEDY_COMMAND)
    {
        return vec![format!(
            "Tell your user: run `{GROK_TOKEN_REFRESH_REMEDY_COMMAND}` once so the Grok CLI can refresh its own session token. quota-axi delegates that refresh to the Grok CLI and never rotates credentials itself."
        )];
    }
    Vec::new()
}
