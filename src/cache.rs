//! On-disk provider quota cache, mirroring `src/cache.ts`.
//!
//! Fresh readings whose windows are non-empty are stamped (and, for
//! context-scoped providers, credential-tagged) and persisted atomically so a
//! later `stale` reuse can prove the numbers came from the same account.

use std::collections::{HashMap, HashSet};
use std::path::Path;

use serde_json::{Map, Value};

use crate::lib::fs::{cache_file_path, ensure_private_parent, read_json_file};
use crate::lib::time::now_iso;
use crate::types::{
    Credits, CreditsUnit, ProviderId, ProviderQuota, ProviderSource, ProviderState, ProviderStatus,
    QuotaWindow, WindowKind, PROVIDER_IDS,
};

const CACHE_SCHEMA_VERSION: i64 = 2;

/// A cache record: the normalized snapshot plus, for context-scoped providers,
/// the credential context the reading was captured under.
struct CachedProvider {
    snapshot: ProviderQuota,
    credential_context_id: Option<String>,
}

/// Whether a string is a valid credential context id (`^[a-f0-9]{64}$`).
fn is_credential_context_id(value: &str) -> bool {
    value.len() == 64
        && value
            .chars()
            .all(|c| c.is_ascii_digit() || ('a'..='f').contains(&c))
}

/// Providers whose local configuration decides which account a reading belongs
/// to: a Claude profile selects the credential store, and a Kimi Code
/// `config.toml` selects the deployment. A snapshot from one such context says
/// nothing about another, so each is stamped on write and required to match on
/// stale reuse.
fn is_context_scoped(provider: ProviderId) -> bool {
    // wired with the claude/kimi provider port
    matches!(provider, ProviderId::Claude | ProviderId::Kimi)
}

/// Resolve the credential context stamp for a provider. Claude and Kimi both
/// require their own credential-discovery logic, which lands with their ports.
fn credential_context_id_for(_provider: ProviderId) -> Option<String> {
    // wired with the claude/kimi provider port
    None
}

pub fn read_cached_provider(provider: ProviderId) -> Option<ProviderQuota> {
    read_cache_providers()
        .into_iter()
        .find(|item| item.snapshot.provider == provider)
        .map(|item| item.snapshot)
}

pub fn delete_cached_provider(provider: ProviderId) {
    let existing = read_cache_providers();
    if !existing.iter().any(|item| item.snapshot.provider == provider) {
        return;
    }
    let remaining: Vec<CachedProvider> = existing
        .into_iter()
        .filter(|item| item.snapshot.provider != provider)
        .collect();
    write_cache_file(&cache_file_path(), &remaining);
}

pub fn write_cached_providers(providers: &[ProviderQuota]) {
    let clear_providers: HashSet<ProviderId> = providers
        .iter()
        .filter(|provider| {
            provider.state.status == ProviderStatus::Fresh && provider.windows.is_empty()
        })
        .map(|provider| provider.provider)
        .collect();
    let cacheable: Vec<CachedProvider> = providers.iter().filter_map(to_cache_provider).collect();

    let file = cache_file_path();
    let mut by_provider: HashMap<ProviderId, CachedProvider> = HashMap::new();
    let mut cleared_existing = false;
    for provider in read_cache_providers() {
        if clear_providers.contains(&provider.snapshot.provider) {
            cleared_existing = true;
            continue;
        }
        by_provider.insert(provider.snapshot.provider, provider);
    }
    if cacheable.is_empty() && !cleared_existing {
        return;
    }
    for provider in cacheable {
        by_provider.insert(provider.snapshot.provider, provider);
    }
    let merged: Vec<CachedProvider> = PROVIDER_IDS
        .iter()
        .filter_map(|id| by_provider.remove(id))
        .collect();

    write_cache_file(&file, &merged);
}

/// Write `providers` to `file` atomically: private parent, 0600 temp, rename,
/// 0600 result. Best-effort — an IO failure never fails the report.
fn write_cache_file(file: &Path, providers: &[CachedProvider]) {
    ensure_private_parent(file);
    let temp = format!("{}.{}.tmp", file.display(), std::process::id());
    let temp_path = Path::new(&temp);

    let generated_at = now_iso();
    let payload = serde_json::json!({
        "generatedAt": generated_at,
        "schemaVersion": CACHE_SCHEMA_VERSION,
        "providers": providers.iter().map(serialize_cached_provider).collect::<Vec<_>>(),
    });
    let Ok(json) = serde_json::to_string_pretty(&payload) else {
        return;
    };
    let mut contents = json;
    contents.push('\n');

    let _ = write_file_private(temp_path, contents.as_bytes());
    let _ = set_mode_0600(temp_path);
    let _ = std::fs::rename(temp_path, file);
    let _ = set_mode_0600(file);
}

fn read_cache_providers() -> Vec<CachedProvider> {
    let Some(raw) = read_json_file(&cache_file_path()) else {
        return Vec::new();
    };
    let Some(payload) = raw.as_object() else {
        return Vec::new();
    };
    let schema_version = number_value(payload.get("schemaVersion"));
    let providers_array = payload.get("providers").and_then(Value::as_array);
    let (Some(schema_version), Some(providers_array)) = (schema_version, providers_array) else {
        return Vec::new();
    };
    if schema_version != 1.0 && schema_version != CACHE_SCHEMA_VERSION as f64 {
        return Vec::new();
    }
    providers_array
        .iter()
        .filter_map(|provider| normalize_cached_provider(provider, schema_version))
        .collect()
}

fn to_cache_provider(provider: &ProviderQuota) -> Option<CachedProvider> {
    if provider.state.status != ProviderStatus::Fresh || provider.windows.is_empty() {
        return None;
    }
    // Round-trip through the normalizer so the snapshot only ever carries the
    // fields the cache reader understands (mirrors `toCacheProvider`).
    let raw = serde_json::json!({
        "provider": provider.provider,
        "label": provider.label,
        "source": provider.source,
        "plan": provider.plan,
        "windows": provider.windows,
        "credits": provider.credits,
        "state": {
            "status": provider.state.status,
            "stale": false,
            "refreshedAt": provider.state.refreshed_at,
            "untrustedWindowIds": provider.state.untrusted_window_ids,
            "sourcesTried": provider.state.sources_tried,
        },
    });
    let snapshot = normalize_cached_provider(&raw, CACHE_SCHEMA_VERSION as f64)?.snapshot;
    let context_id = credential_context_id_for(provider.provider);
    Some(CachedProvider {
        snapshot,
        credential_context_id: context_id,
    })
}

fn serialize_cached_provider(provider: &CachedProvider) -> Value {
    let mut value = serde_json::to_value(&provider.snapshot).unwrap_or(Value::Null);
    if let Some(context) = &provider.credential_context_id {
        if let Value::Object(map) = &mut value {
            map.insert(
                "credentialContext".to_string(),
                Value::String(context.clone()),
            );
        }
    }
    value
}

fn normalize_cached_provider(raw: &Value, schema_version: f64) -> Option<CachedProvider> {
    let data = raw.as_object()?;
    let provider = literal_provider(data.get("provider"));
    let label = string_value(data.get("label"));
    let source = literal_source(data.get("source"));
    let state = object_value(data.get("state"));
    let status = state.and_then(|s| literal_status(s.get("status")));
    let sources_tried = state.and_then(|s| string_array_value(s.get("sourcesTried")));
    let windows: Vec<QuotaWindow> = data
        .get("windows")
        .and_then(Value::as_array)
        .map(|arr| arr.iter().filter_map(normalize_cached_window).collect())
        .unwrap_or_default();

    if provider.is_none()
        || label.is_none()
        || source.is_none()
        || state.is_none()
        || status.is_none()
        || sources_tried.is_none()
        || windows.is_empty()
        || (provider == Some(ProviderId::Codex) && has_invalid_codex_window_identities(&windows))
    {
        return None;
    }

    let provider = provider.unwrap();
    let state = state.unwrap();
    let mut snapshot = ProviderQuota {
        provider,
        label: Some(label.unwrap()),
        source: Some(source.unwrap()),
        plan: None,
        account: None,
        windows,
        quota_semantics: None,
        credits: None,
        state: ProviderState {
            status: status.unwrap(),
            stale: boolean_value(state.get("stale")).unwrap_or(false),
            refreshed_at: None,
            error: None,
            retry_after: None,
            auth_status: None,
            reason: None,
            remedy_command: None,
            untrusted_window_ids: None,
            degraded_sources: None,
            sources_tried: Some(sources_tried.unwrap()),
        },
        attempts: None,
    };
    if let Some(plan) = string_value(data.get("plan")) {
        snapshot.plan = Some(plan);
    }
    if let Some(refreshed_at) = string_value(state.get("refreshedAt")) {
        snapshot.state.refreshed_at = Some(refreshed_at);
    }
    if let Some(untrusted) = string_array_value(state.get("untrustedWindowIds")) {
        snapshot.state.untrusted_window_ids = Some(untrusted);
    }
    if let Some(credits) = normalize_cached_credits(data.get("credits")) {
        snapshot.credits = Some(credits);
    }

    let credential_context = string_value(data.get("credentialContext"));
    let credential_context_id = if schema_version == CACHE_SCHEMA_VERSION as f64
        && is_context_scoped(provider)
        && credential_context.is_some()
        && is_credential_context_id(credential_context.as_deref().unwrap_or_default())
    {
        credential_context
    } else {
        None
    };

    Some(CachedProvider {
        snapshot,
        credential_context_id,
    })
}

/// A Codex window id must be its base identity (or a `_2`, `_3`, ... disambiguator),
/// and every window must share the same base identity naming scheme. Returns true
/// when any window violates that contract.
fn has_invalid_codex_window_identities(windows: &[QuotaWindow]) -> bool {
    let mut counts: HashMap<String, usize> = HashMap::new();
    for window in windows {
        let Some(base_id) = codex_window_base_identity(window) else {
            return true;
        };
        let count = counts.entry(base_id.clone()).or_insert(0);
        *count += 1;
        let expected = if *count == 1 {
            base_id.clone()
        } else {
            format!("{base_id}_{count}")
        };
        if window.id != expected {
            return true;
        }
    }
    false
}

fn codex_window_base_identity(window: &QuotaWindow) -> Option<String> {
    let id = strip_codex_suffix(&window.id);
    match window.window_seconds {
        None => {
            if matches_window_identity(window, &id, "five_hour", "session", WindowKind::Session) {
                return Some(id);
            }
            if matches_window_identity(window, &id, "weekly", "week", WindowKind::Weekly) {
                return Some(id);
            }
            if matches_window_identity(
                window,
                &id,
                "code_review_five_hour",
                "code review session",
                WindowKind::Session,
            ) || matches_window_identity(
                window,
                &id,
                "code_review_weekly",
                "code review week",
                WindowKind::Weekly,
            ) || matches_model_window_identity(window, &id, "5h", "session")
                || matches_model_window_identity(window, &id, "7d", "week")
            {
                return Some(id);
            }
            None
        }
        Some(seconds) if seconds == 18_000.0 => {
            if matches_window_identity(window, &id, "five_hour", "session", WindowKind::Session)
                || matches_window_identity(
                    window,
                    &id,
                    "code_review_five_hour",
                    "code review session",
                    WindowKind::Session,
                )
                || matches_model_window_identity(window, &id, "5h", "session")
            {
                return Some(id);
            }
            None
        }
        Some(seconds) if seconds == 604_800.0 => {
            if matches_window_identity(window, &id, "weekly", "week", WindowKind::Weekly)
                || matches_window_identity(
                    window,
                    &id,
                    "code_review_weekly",
                    "code review week",
                    WindowKind::Weekly,
                )
                || matches_model_window_identity(window, &id, "7d", "week")
            {
                return Some(id);
            }
            None
        }
        Some(seconds) => {
            let duration = readable_window_duration(seconds);
            if matches_window_identity(
                window,
                &id,
                &format!("window:{duration}"),
                &format!("{duration} window"),
                WindowKind::Unknown,
            ) || matches_window_identity(
                window,
                &id,
                &format!("code_review_window:{duration}"),
                &format!("{duration} window"),
                WindowKind::Unknown,
            ) || matches_model_window_identity(
                window,
                &id,
                &format!("window:{duration}"),
                &format!("{duration} window"),
            ) {
                return Some(id);
            }
            None
        }
    }
}

fn matches_window_identity(
    window: &QuotaWindow,
    actual_id: &str,
    expected_id: &str,
    label: &str,
    kind: WindowKind,
) -> bool {
    actual_id == expected_id && window.label == label && window.kind == kind
}

fn matches_model_window_identity(
    window: &QuotaWindow,
    id: &str,
    suffix: &str,
    label_suffix: &str,
) -> bool {
    id.starts_with("model:")
        && id.ends_with(&format!(":{suffix}"))
        && id.len() > format!("model::{suffix}").len()
        && window.label.ends_with(&format!(" {label_suffix}"))
        && window.label.len() > label_suffix.len() + 1
        && window.kind == WindowKind::Model
}

/// Strip a trailing `_N` (N in 2..) disambiguator from a Codex window id.
fn strip_codex_suffix(id: &str) -> String {
    if let Some(pos) = id.rfind('_') {
        let suffix = &id[pos + 1..];
        if is_codex_suffix(suffix) {
            return id[..pos].to_string();
        }
    }
    id.to_string()
}

/// `[2-9]\d*` — a codex disambiguator is a digit 2-9 followed by zero or more digits.
fn is_codex_suffix(suffix: &str) -> bool {
    let mut chars = suffix.chars();
    match chars.next() {
        Some(c) if ('2'..='9').contains(&c) => {}
        _ => return false,
    }
    chars.all(|c| c.is_ascii_digit())
}

fn readable_window_duration(window_seconds: f64) -> String {
    let hours = window_seconds / 3600.0;
    let rounded = (hours * 100.0).round() / 100.0;
    format!("{rounded}h")
}

fn normalize_cached_window(raw: &Value) -> Option<QuotaWindow> {
    let data = raw.as_object()?;
    let id = string_value(data.get("id"))?;
    let label = string_value(data.get("label"))?;
    let kind = literal_window_kind(data.get("kind"))?;
    Some(QuotaWindow {
        id,
        label,
        kind,
        percent_used: number_value(data.get("percentUsed")),
        percent_remaining: number_value(data.get("percentRemaining")),
        starts_at: string_value(data.get("startsAt")),
        resets_at: string_value(data.get("resetsAt")),
        reset_text: string_value(data.get("resetText")),
        window_seconds: number_value(data.get("windowSeconds")),
        spent_usd: number_value(data.get("spentUsd")),
        limit_usd: number_value(data.get("limitUsd")),
        pace: None,
    })
}

fn normalize_cached_credits(raw: Option<&Value>) -> Option<Credits> {
    let data = object_value(raw)?;
    let remaining = number_value(data.get("remaining"));
    let unlimited = boolean_value(data.get("unlimited"));
    let unit = literal_credits_unit(data.get("unit"));
    if remaining.is_none() && unlimited.is_none() && unit.is_none() {
        return None;
    }
    Some(Credits {
        remaining,
        unlimited,
        unit,
    })
}

fn object_value(value: Option<&Value>) -> Option<&Map<String, Value>> {
    value?.as_object()
}

fn string_value(value: Option<&Value>) -> Option<String> {
    match value?.as_str() {
        Some(s) if !s.is_empty() => Some(s.to_string()),
        _ => None,
    }
}

fn number_value(value: Option<&Value>) -> Option<f64> {
    value?.as_f64().filter(|f| f.is_finite())
}

fn boolean_value(value: Option<&Value>) -> Option<bool> {
    value?.as_bool()
}

fn string_array_value(value: Option<&Value>) -> Option<Vec<String>> {
    let arr = value?.as_array()?;
    let mut out = Vec::with_capacity(arr.len());
    for item in arr {
        out.push(item.as_str()?.to_string());
    }
    Some(out)
}

fn literal_provider(value: Option<&Value>) -> Option<ProviderId> {
    ProviderId::from_str(value?.as_str()?)
}

fn literal_source(value: Option<&Value>) -> Option<ProviderSource> {
    match value?.as_str()? {
        "oauth" => Some(ProviderSource::Oauth),
        "pi:openai-codex" => Some(ProviderSource::PiOpenaiCodex),
        "cli-rpc" => Some(ProviderSource::CliRpc),
        "cli" => Some(ProviderSource::Cli),
        "api" => Some(ProviderSource::Api),
        "web" => Some(ProviderSource::Web),
        "cache" => Some(ProviderSource::Cache),
        "unavailable" => Some(ProviderSource::Unavailable),
        _ => None,
    }
}

fn literal_status(value: Option<&Value>) -> Option<ProviderStatus> {
    match value?.as_str()? {
        "fresh" => Some(ProviderStatus::Fresh),
        "stale" => Some(ProviderStatus::Stale),
        "unavailable" => Some(ProviderStatus::Unavailable),
        "auth_required" => Some(ProviderStatus::AuthRequired),
        "rate_limited" => Some(ProviderStatus::RateLimited),
        "error" => Some(ProviderStatus::Error),
        _ => None,
    }
}

fn literal_window_kind(value: Option<&Value>) -> Option<WindowKind> {
    match value?.as_str()? {
        "session" => Some(WindowKind::Session),
        "weekly" => Some(WindowKind::Weekly),
        "monthly" => Some(WindowKind::Monthly),
        "model" => Some(WindowKind::Model),
        "credits" => Some(WindowKind::Credits),
        "unknown" => Some(WindowKind::Unknown),
        _ => None,
    }
}

fn literal_credits_unit(value: Option<&Value>) -> Option<CreditsUnit> {
    match value?.as_str()? {
        "usd" => Some(CreditsUnit::Usd),
        "credits" => Some(CreditsUnit::Credits),
        _ => None,
    }
}

#[cfg(unix)]
fn write_file_private(path: &Path, contents: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(path)?;
    file.write_all(contents)
}

#[cfg(not(unix))]
fn write_file_private(path: &Path, contents: &[u8]) -> std::io::Result<()> {
    std::fs::write(path, contents)
}

#[cfg(unix)]
fn set_mode_0600(path: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
}

#[cfg(not(unix))]
fn set_mode_0600(_path: &Path) -> std::io::Result<()> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_codex_disambiguator_suffix() {
        assert_eq!(strip_codex_suffix("five_hour_2"), "five_hour");
        assert_eq!(strip_codex_suffix("weekly_29"), "weekly");
        assert_eq!(strip_codex_suffix("five_hour"), "five_hour");
        // `_1` and `_0` are not codex disambiguators.
        assert_eq!(strip_codex_suffix("weekly_1"), "weekly_1");
        assert_eq!(strip_codex_suffix("weekly_0"), "weekly_0");
        assert_eq!(strip_codex_suffix("weekly_2x"), "weekly_2x");
    }

    #[test]
    fn validates_credential_context_id_shape() {
        assert!(is_credential_context_id(&"a".repeat(64)));
        assert!(is_credential_context_id(&"0123456789abcdef".repeat(4)));
        assert!(!is_credential_context_id(&"a".repeat(63)));
        assert!(!is_credential_context_id(&"g".repeat(64)));
        assert!(!is_credential_context_id(&"A".repeat(64)));
    }
}
