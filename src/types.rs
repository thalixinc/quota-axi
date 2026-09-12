//! Normalized report model — the single Rust mirror of `src/types.ts` (schemaVersion 5).
//! Field order and omission (`skip_serializing_if`) are part of the `--json`/`--full`
//! contract and must match the Node renderer byte-for-byte.

use serde::{Deserialize, Serialize};

/// Serialize a numeric value the way `JSON.stringify` does: an integral float
/// (e.g. `58.0`) emits `58`, matching the Node build's `--json` output.
pub mod serde_num {
    use serde::Serializer;

    pub fn serialize<S: Serializer>(value: &f64, serializer: S) -> Result<S::Ok, S::Error> {
        if value.is_finite() && value.fract() == 0.0 && value.abs() < 9e15 {
            serializer.serialize_i64(*value as i64)
        } else {
            serializer.serialize_f64(*value)
        }
    }

    pub fn serialize_opt<S: Serializer>(
        value: &Option<f64>,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        match value {
            Some(v) => serialize(v, serializer),
            None => serializer.serialize_none(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ProviderId {
    Claude,
    Codex,
    Cursor,
    Copilot,
    Grok,
    Kimi,
    Zai,
    Agy,
    Alibaba,
    #[serde(rename = "opencode-go")]
    OpencodeGo,
}

pub const PROVIDER_IDS: [ProviderId; 10] = [
    ProviderId::Claude,
    ProviderId::Codex,
    ProviderId::Cursor,
    ProviderId::Copilot,
    ProviderId::Grok,
    ProviderId::Kimi,
    ProviderId::Zai,
    ProviderId::Agy,
    ProviderId::Alibaba,
    ProviderId::OpencodeGo,
];

impl ProviderId {
    pub fn as_str(self) -> &'static str {
        match self {
            ProviderId::Claude => "claude",
            ProviderId::Codex => "codex",
            ProviderId::Cursor => "cursor",
            ProviderId::Copilot => "copilot",
            ProviderId::Grok => "grok",
            ProviderId::Kimi => "kimi",
            ProviderId::Zai => "zai",
            ProviderId::Agy => "agy",
            ProviderId::Alibaba => "alibaba",
            ProviderId::OpencodeGo => "opencode-go",
        }
    }

    pub fn from_str(value: &str) -> Option<Self> {
        Some(match value {
            "claude" => ProviderId::Claude,
            "codex" => ProviderId::Codex,
            "cursor" => ProviderId::Cursor,
            "copilot" => ProviderId::Copilot,
            "grok" => ProviderId::Grok,
            "kimi" => ProviderId::Kimi,
            "zai" => ProviderId::Zai,
            "agy" => ProviderId::Agy,
            "alibaba" => ProviderId::Alibaba,
            "opencode-go" => ProviderId::OpencodeGo,
            _ => return None,
        })
    }
}

impl std::fmt::Display for ProviderId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ProviderSource {
    Oauth,
    #[serde(rename = "pi:openai-codex")]
    PiOpenaiCodex,
    #[serde(rename = "cli-rpc")]
    CliRpc,
    Cli,
    Api,
    Web,
    Cache,
    Unavailable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderStatus {
    Fresh,
    Stale,
    Unavailable,
    #[serde(rename = "auth_required")]
    AuthRequired,
    #[serde(rename = "rate_limited")]
    RateLimited,
    Error,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderAuthStatus {
    Usable,
    #[serde(rename = "expired_refreshable")]
    ExpiredRefreshable,
    Unusable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderStateReason {
    #[serde(rename = "keychain_access_required")]
    KeychainAccessRequired,
    #[serde(rename = "credentials_expired")]
    CredentialsExpired,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum QuotaPaceStatus {
    Ahead,
    #[serde(rename = "on_pace")]
    OnPace,
    Behind,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum QuotaPaceReason {
    Stale,
    #[serde(rename = "missing_usage")]
    MissingUsage,
    #[serde(rename = "missing_cycle")]
    MissingCycle,
    #[serde(rename = "invalid_cycle")]
    InvalidCycle,
    #[serde(rename = "future_cycle_start")]
    FutureCycleStart,
    #[serde(rename = "expired_reset")]
    ExpiredReset,
    #[serde(rename = "unsupported_period")]
    UnsupportedPeriod,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct QuotaPace {
    pub status: QuotaPaceStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<QuotaPaceReason>,
    #[serde(skip_serializing_if = "Option::is_none", serialize_with = "serde_num::serialize_opt")]
    pub time_remaining_percent: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none", serialize_with = "serde_num::serialize_opt")]
    pub elapsed_percent: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none", serialize_with = "serde_num::serialize_opt")]
    pub reserve_percent_points: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none", serialize_with = "serde_num::serialize_opt")]
    pub burn_multiple: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub projected_exhausted_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub projection_confidence: Option<ProjectionConfidence>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cycle_basis: Option<CycleBasis>,
    #[serde(skip_serializing_if = "Option::is_none", serialize_with = "serde_num::serialize_opt")]
    pub cycle_seconds: Option<f64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ProjectionConfidence {
    Early,
    Established,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CycleBasis {
    #[serde(rename = "starts_at_resets_at")]
    StartsAtResetsAt,
    #[serde(rename = "window_seconds")]
    WindowSeconds,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunwayStatus {
    #[serde(rename = "exhausted_now")]
    ExhaustedNow,
    #[serde(rename = "projected_exhaustion")]
    ProjectedExhaustion,
    #[serde(rename = "through_reset")]
    ThroughReset,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EffectiveRunway {
    pub status: RunwayStatus,
    #[serde(skip_serializing_if = "Option::is_none", serialize_with = "serde_num::serialize_opt")]
    pub usable_runway_seconds: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub projected_exhausted_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub limiting_window_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub projection_confidence: Option<ProjectionConfidence>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub unmeasurable_window_ids: Option<Vec<String>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PaceSummaryStatus {
    Ahead,
    #[serde(rename = "on_pace")]
    OnPace,
    Behind,
    Mixed,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EffectivePaceSummary {
    pub status: PaceSummaryStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ahead_window_ids: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub behind_window_ids: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub on_pace_window_ids: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub unknown_window_ids: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none", serialize_with = "serde_num::serialize_opt")]
    pub worst_reserve_percent_points: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub worst_reserve_window_id: Option<String>,
}

pub const SELECTION_SCALAR_KEY: &str = "spendPriority";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EffectiveSelection {
    pub status: SelectionStatus,
    #[serde(skip_serializing_if = "Option::is_none", serialize_with = "serde_num::serialize_opt")]
    pub spend_priority: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub unmeasurable_window_ids: Option<Vec<String>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SelectionStatus {
    Known,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BoundConflict {
    pub exhausted_window_ids: Vec<String>,
    pub live_window_ids: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WindowKind {
    Session,
    Weekly,
    Monthly,
    Model,
    Credits,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct QuotaWindow {
    pub id: String,
    pub label: String,
    pub kind: WindowKind,
    #[serde(skip_serializing_if = "Option::is_none", serialize_with = "serde_num::serialize_opt")]
    pub percent_used: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none", serialize_with = "serde_num::serialize_opt")]
    pub percent_remaining: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub starts_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resets_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reset_text: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", serialize_with = "serde_num::serialize_opt")]
    pub window_seconds: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none", serialize_with = "serde_num::serialize_opt")]
    pub spent_usd: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none", serialize_with = "serde_num::serialize_opt")]
    pub limit_usd: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pace: Option<QuotaPace>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AvailabilityStatus {
    Known,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EffectiveAvailability {
    pub scope: String,
    pub status: AvailabilityStatus,
    #[serde(skip_serializing_if = "Option::is_none", serialize_with = "serde_num::serialize_opt")]
    pub effective_percent_remaining: Option<f64>,
    pub bounded_by: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub limiting_window_ids: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bound_conflict: Option<BoundConflict>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pace: Option<EffectivePaceSummary>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub runway: Option<EffectiveRunway>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub selection: Option<EffectiveSelection>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SemanticsStatus {
    Known,
    Partial,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct QuotaSemantics {
    pub status: SemanticsStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub effective_availability: Vec<EffectiveAvailability>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub unresolved_window_ids: Option<Vec<String>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AttemptStatus {
    Success,
    Failed,
    Skipped,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceAttempt {
    pub source: String,
    pub status: AttemptStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub credential_present: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub degraded: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DegradedSource {
    pub source: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Account {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub organization: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub account_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub identity_status: Option<IdentityStatus>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum IdentityStatus {
    Verified,
    Unverified,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Credits {
    #[serde(skip_serializing_if = "Option::is_none", serialize_with = "serde_num::serialize_opt")]
    pub remaining: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub unlimited: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub unit: Option<CreditsUnit>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CreditsUnit {
    Usd,
    Credits,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderState {
    pub status: ProviderStatus,
    pub stale: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub refreshed_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub retry_after: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub auth_status: Option<ProviderAuthStatus>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<ProviderStateReason>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub remedy_command: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub untrusted_window_ids: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub degraded_sources: Option<Vec<DegradedSource>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sources_tried: Option<Vec<String>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderQuota {
    pub provider: ProviderId,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<ProviderSource>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub plan: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub account: Option<Account>,
    pub windows: Vec<QuotaWindow>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub credits: Option<Credits>,
    pub state: ProviderState,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub attempts: Option<Vec<SourceAttempt>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub quota_semantics: Option<QuotaSemantics>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct QuotaAxiResponse {
    pub generated_at: String,
    pub schema_version: i32,
    pub providers: Vec<ProviderQuota>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub help: Option<Vec<String>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderOptions {
    pub allow_keychain_prompt: bool,
    pub refresh_credentials: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AuthSourceReport {
    pub source: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    pub status: AuthSourceStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub credential_present: Option<bool>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AuthSourceStatus {
    Available,
    Missing,
    Invalid,
    Expired,
    Skipped,
    Error,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AuthProviderReport {
    pub provider: ProviderId,
    pub sources: Vec<AuthSourceReport>,
}

pub trait ProviderAdapter: Sync + Send {
    fn id(&self) -> ProviderId;
    fn label(&self) -> &'static str;
    fn fetch_quota(&self, options: &ProviderOptions) -> ProviderQuota;
    fn inspect_auth(&self, options: &ProviderOptions) -> AuthProviderReport;
}

