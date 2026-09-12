//! Antigravity (`agy`) provider adapter, mirroring `src/providers/agy.ts`.
//!
//! Antigravity is a local language-server that publishes quota over a loopback
//! gRPC-web endpoint. This adapter discovers the running process, finds its
//! listening port, and reads quota via unauthenticated loopback HTTP(S) POST.

use std::io::Read;
use std::time::{Duration, Instant};

use serde_json::{Map, Value};

use crate::cache::{delete_cached_provider, read_cached_provider};
use crate::lib::process::{current_user_process_list_args, effective_uid, exec_file_text};
use crate::lib::time::{clamp_percent, now_iso, parse_epoch_or_iso, percent_remaining};
use crate::providers::{
    failed_provider, source_names, stale_from_cache, status_from_error, success_provider,
    SuccessArgs,
};
use crate::types::{
    Account, AttemptStatus, AuthProviderReport, AuthSourceReport, AuthSourceStatus,
    ProviderAdapter, ProviderId, ProviderOptions, ProviderQuota, ProviderSource, ProviderStatus,
    QuotaWindow, SourceAttempt, WindowKind,
};

const QUOTA_SUMMARY_PATH: &str =
    "/exa.language_server_pb.LanguageServerService/RetrieveUserQuotaSummary";
const USER_STATUS_PATH: &str = "/exa.language_server_pb.LanguageServerService/GetUserStatus";
const COMMAND_MODEL_CONFIGS_PATH: &str =
    "/exa.language_server_pb.LanguageServerService/GetCommandModelConfigs";
const UNLEASH_PATH: &str = "/exa.language_server_pb.LanguageServerService/GetUnleashData";
const PROCESS_TIMEOUT_MS: u64 = 5_000;
const PORT_TIMEOUT_MS: u64 = 2_000;
const REQUEST_TIMEOUT_MS: u64 = 3_000;
const PROBE_BUDGET_MS: u64 = 10_000;
const MAX_RESPONSE_BYTES: usize = 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgyProcessSource {
    Agy,
    App,
}

#[derive(Debug, Clone)]
pub struct AgyProcessInfo {
    pub pid: u32,
    pub command: String,
    pub source: AgyProcessSource,
    pub csrf_token: Option<String>,
    pub extension_port: Option<u32>,
    pub extension_server_csrf_token: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgyScheme {
    Https,
    Http,
}

#[derive(Debug, Clone)]
pub struct AgyConnectionEndpoint {
    pub scheme: AgyScheme,
    pub port: u32,
    pub source: AgyProcessSource,
    pub pid: u32,
    pub csrf_token: Option<String>,
    pub requires_csrf_token: bool,
    pub requires_unleash_probe: bool,
}

/// An injectable probe surface so the adapter can be driven without a real
/// Antigravity install. The default runtime shells out to `ps`/`lsof` and posts
/// to the loopback endpoint.
pub trait AgyProbeRuntime {
    fn exec_file_text(&self, command: &str, args: &[String], timeout_ms: u64)
        -> Result<String, String>;
    fn request_json(
        &self,
        endpoint: &AgyConnectionEndpoint,
        path: &str,
        timeout_ms: u64,
    ) -> Result<Value, AgyError>;
}

/// The classified failure surface for the adapter's async (here: sync) flow.
#[derive(Debug, Clone, PartialEq)]
pub enum AgyError {
    Unavailable(String),
    ProbeBudget,
    Malformed(String),
    Discovery(String),
    Http(u32),
    Other(String),
}

impl AgyError {
    pub fn message(&self) -> String {
        match self {
            AgyError::Unavailable(m)
            | AgyError::Malformed(m)
            | AgyError::Discovery(m)
            | AgyError::Other(m) => m.clone(),
            AgyError::ProbeBudget => "Antigravity probe timed out".to_string(),
            AgyError::Http(status) => http_error_message(*status),
        }
    }

    fn is_unavailable(&self) -> bool {
        matches!(self, AgyError::Unavailable(_) | AgyError::ProbeBudget)
    }

    fn is_probe_budget(&self) -> bool {
        matches!(self, AgyError::ProbeBudget)
    }
}

pub struct AgyAdapter;
pub static AGY: &'static dyn ProviderAdapter = &AgyAdapter;

impl ProviderAdapter for AgyAdapter {
    fn id(&self) -> ProviderId {
        ProviderId::Agy
    }

    fn label(&self) -> &'static str {
        "Antigravity"
    }

    fn fetch_quota(&self, _options: &ProviderOptions) -> ProviderQuota {
        fetch_quota_with_runtime(&DefaultRuntime)
    }

    fn inspect_auth(&self, _options: &ProviderOptions) -> AuthProviderReport {
        inspect_auth_with_runtime(&DefaultRuntime)
    }
}

struct DefaultRuntime;

impl AgyProbeRuntime for DefaultRuntime {
    fn exec_file_text(
        &self,
        command: &str,
        args: &[String],
        timeout_ms: u64,
    ) -> Result<String, String> {
        exec_file_text(command, args, timeout_ms)
    }

    fn request_json(
        &self,
        endpoint: &AgyConnectionEndpoint,
        path: &str,
        timeout_ms: u64,
    ) -> Result<Value, AgyError> {
        request_loopback_json(endpoint, path, timeout_ms)
    }
}

struct QuotaResult {
    plan: Option<String>,
    account: Option<Account>,
    windows: Vec<QuotaWindow>,
    refreshed_at: String,
}

struct IdentityResult {
    plan: Option<String>,
    account: Option<Account>,
}

pub struct QuotaSummaryResult {
    pub windows: Vec<QuotaWindow>,
    pub refreshed_at: String,
}

pub struct UserStatusResult {
    pub plan: Option<String>,
    pub account: Option<Account>,
    pub windows: Vec<QuotaWindow>,
    pub refreshed_at: String,
}

struct WindowGroup {
    id: String,
    label: String,
}

struct WindowKindInfo {
    id: String,
    label: String,
    kind: WindowKind,
}

pub fn fetch_quota_with_runtime(runtime: &dyn AgyProbeRuntime) -> ProviderQuota {
    let mut attempts: Vec<SourceAttempt> = vec![SourceAttempt {
        source: "loopback".to_string(),
        status: AttemptStatus::Failed,
        error: None,
        credential_present: None,
        degraded: None,
    }];

    let final_failure = match fetch_loopback_quota(runtime) {
        Ok(quota) => {
            attempts[0].status = AttemptStatus::Success;
            return success_provider(SuccessArgs {
                provider: ProviderId::Agy,
                label: "Antigravity".to_string(),
                source: ProviderSource::CliRpc,
                plan: quota.plan,
                account: quota.account,
                windows: quota.windows,
                credits: None,
                refreshed_at: quota.refreshed_at,
                sources_tried: source_names(&attempts),
                attempts: Some(attempts),
            });
        }
        Err(error) => {
            let final_error = error.message();
            attempts[0] = SourceAttempt {
                source: "loopback".to_string(),
                status: if error.is_unavailable() {
                    AttemptStatus::Skipped
                } else {
                    AttemptStatus::Failed
                },
                error: Some(final_error),
                credential_present: None,
                degraded: None,
            };
            Some(error)
        }
    };

    let final_error = final_failure
        .as_ref()
        .map(AgyError::message)
        .unwrap_or_else(|| "Antigravity quota unavailable".to_string());

    if final_failure
        .as_ref()
        .map(stale_eligible_failure)
        .unwrap_or(false)
    {
        if let Some(cached) = read_cached_provider(ProviderId::Agy) {
            return stale_from_cache(cached, &final_error, source_names(&attempts), attempts);
        }
    } else if final_failure
        .as_ref()
        .map(is_definitive_auth_failure)
        .unwrap_or(false)
    {
        // Cache retirement is best effort; preserve the definitive auth result.
        delete_cached_provider(ProviderId::Agy);
    }

    failed_provider(
        ProviderId::Agy,
        "Antigravity",
        status_for_error(&final_error),
        &final_error,
        source_names(&attempts),
        None,
        None,
        Some(attempts),
    )
}

pub fn inspect_auth_with_runtime(runtime: &dyn AgyProbeRuntime) -> AuthProviderReport {
    match discover_agy_endpoints(runtime, create_probe_deadline()) {
        Ok(endpoints) => AuthProviderReport {
            provider: ProviderId::Agy,
            sources: vec![AuthSourceReport {
                source: "loopback".to_string(),
                path: None,
                status: if endpoints.is_empty() {
                    AuthSourceStatus::Missing
                } else {
                    AuthSourceStatus::Available
                },
                error: None,
                credential_present: None,
            }],
        },
        Err(error) => AuthProviderReport {
            provider: ProviderId::Agy,
            sources: vec![AuthSourceReport {
                source: "loopback".to_string(),
                path: None,
                status: AuthSourceStatus::Error,
                error: Some(error.message()),
                credential_present: None,
            }],
        },
    }
}

pub fn normalize_agy_quota_summary(raw: &Value) -> Option<QuotaSummaryResult> {
    let payload = quota_summary_payload(raw)?;
    let groups = array_value(payload.get("groups"));
    let mut windows: Vec<QuotaWindow> = groups.iter().flat_map(normalize_quota_summary_group).collect();
    windows.sort_by(compare_agy_windows);
    if windows.is_empty() {
        return None;
    }
    Some(QuotaSummaryResult {
        windows,
        refreshed_at: now_iso(),
    })
}

pub fn normalize_agy_user_status(raw: &Value) -> Option<UserStatusResult> {
    let data = raw.as_object()?;
    let status = object_value(data.get("userStatus")).unwrap_or(data);
    let plan_status = object_value(status.get("planStatus"));
    let plan_info = plan_status.and_then(|ps| object_value(ps.get("planInfo")));
    let account_email = string_value(status.get("email"));
    let user_tier = object_value(status.get("userTier"));
    let plan = user_tier
        .and_then(|ut| string_value(ut.get("name")))
        .or_else(|| plan_info.and_then(|pi| string_value(pi.get("planName"))));
    let config_data = object_value(status.get("cascadeModelConfigData"))
        .or_else(|| object_value(data.get("cascadeModelConfigData")))
        .unwrap_or(data);
    let windows = normalize_model_config_windows(config_data);
    if windows.is_empty() && plan.is_none() && account_email.is_none() {
        return None;
    }
    Some(UserStatusResult {
        plan,
        account: account_email.map(|email| Account {
            email: Some(email),
            organization: None,
            account_id: None,
            identity_status: None,
        }),
        windows,
        refreshed_at: now_iso(),
    })
}

pub fn process_infos_from_ps(output: &str) -> Vec<AgyProcessInfo> {
    let mut processes = Vec::new();
    for line in output.split('\n') {
        let line = line.trim_end_matches('\r');
        let Some((pid_str, command)) = split_pid_command(line) else {
            continue;
        };
        let Ok(pid) = pid_str.parse::<u32>() else {
            continue;
        };
        let command = command.trim();
        if command.is_empty() {
            continue;
        }
        let Some(source) = agy_process_source(command) else {
            continue;
        };
        processes.push(AgyProcessInfo {
            pid,
            command: command.to_string(),
            source,
            csrf_token: flag_value(command, "csrf_token"),
            extension_port: flag_value(command, "extension_server_port").and_then(|v| v.parse().ok()),
            extension_server_csrf_token: flag_value(command, "extension_server_csrf_token"),
        });
    }
    processes
}

pub fn ports_from_lsof(output: &str) -> Vec<u32> {
    let mut ports: Vec<u32> = Vec::new();
    for line in output.split('\n') {
        let line = line.trim_end_matches('\r');
        let Some(port) = lsof_listen_port(line) else {
            continue;
        };
        if port > 0 && port <= 65535 && !ports.contains(&port) {
            ports.push(port);
        }
    }
    ports
}

/// `^\s*(\d+)\s+(.+)$` — a leading pid, then whitespace, then the command.
fn split_pid_command(line: &str) -> Option<(&str, &str)> {
    let trimmed = line.trim_start();
    let digit_len = trimmed.chars().take_while(|c| c.is_ascii_digit()).count();
    if digit_len == 0 {
        return None;
    }
    let pid_str = &trimmed[..digit_len];
    let after = &trimmed[digit_len..];
    if !after.chars().next().map(|c| c.is_whitespace()).unwrap_or(false) {
        return None;
    }
    let command = after.trim();
    if command.is_empty() {
        return None;
    }
    Some((pid_str, command))
}

/// `TCP\s+(?:\[[^\]]+\]|[^:]+):(\d+)\s+\(LISTEN\)` — a listening IPv4/IPv6 port.
fn lsof_listen_port(line: &str) -> Option<u32> {
    let tcp_idx = line.find("TCP")?;
    let after_tcp = &line[tcp_idx + "TCP".len()..];
    if !after_tcp
        .chars()
        .next()
        .map(|c| c.is_whitespace())
        .unwrap_or(false)
    {
        return None;
    }
    let after_tcp = after_tcp.trim_start();
    let after_addr = if after_tcp.starts_with('[') {
        let close = after_tcp.find(']')?;
        &after_tcp[close + 1..]
    } else {
        let colon = after_tcp.find(':')?;
        &after_tcp[colon..]
    };
    let rest = after_addr.strip_prefix(':')?;
    let digits_len = rest.chars().take_while(|c| c.is_ascii_digit()).count();
    if digits_len == 0 {
        return None;
    }
    let after_digits = &rest[digits_len..];
    if !after_digits
        .chars()
        .next()
        .map(|c| c.is_whitespace())
        .unwrap_or(false)
    {
        return None;
    }
    if !after_digits.trim_start().starts_with("(LISTEN)") {
        return None;
    }
    rest[..digits_len].parse().ok()
}

fn fetch_loopback_quota(runtime: &dyn AgyProbeRuntime) -> Result<QuotaResult, AgyError> {
    let deadline = create_probe_deadline();
    let endpoints = discover_agy_endpoints(runtime, deadline)?;
    if endpoints.is_empty() {
        return Err(AgyError::Unavailable("Antigravity/agy is not running".to_string()));
    }

    let mut last_error: Option<AgyError> = None;
    for endpoint in &endpoints {
        let result = if endpoint.requires_unleash_probe {
            probe_endpoint(runtime, endpoint, deadline).and_then(|()| {
                fetch_endpoint_quota(runtime, endpoint, deadline)
            })
        } else {
            fetch_endpoint_quota(runtime, endpoint, deadline)
        };
        match result {
            Ok(quota) => return Ok(quota),
            Err(error) => last_error = stronger_failure(last_error, error),
        }
    }

    match last_error {
        Some(error) => Err(error),
        None => Err(AgyError::Other("Antigravity quota unavailable".to_string())),
    }
}

fn discover_agy_endpoints(
    runtime: &dyn AgyProbeRuntime,
    deadline: Instant,
) -> Result<Vec<AgyConnectionEndpoint>, AgyError> {
    let processes = process_infos_from_ps(&read_process_list(runtime, deadline)?);
    let mut endpoints: Vec<AgyConnectionEndpoint> = Vec::new();
    let mut discovery_error: Option<AgyError> = None;

    for process_info in &processes {
        let listening_ports = match read_listening_ports(runtime, process_info.pid, deadline) {
            Ok(ports) => ports,
            Err(error) => {
                if error.is_probe_budget() {
                    return Err(error);
                }
                discovery_error = stronger_failure(discovery_error, error);
                continue;
            }
        };

        if process_info.source == AgyProcessSource::Agy {
            for &port in &listening_ports {
                endpoints.push(endpoint_for(process_info, AgyScheme::Https, port, None, false, false));
                endpoints.push(endpoint_for(process_info, AgyScheme::Http, port, None, false, false));
            }
            continue;
        }

        if let Some(csrf_token) = &process_info.csrf_token {
            for &port in &listening_ports {
                endpoints.push(endpoint_for(
                    process_info,
                    AgyScheme::Https,
                    port,
                    Some(csrf_token.clone()),
                    true,
                    true,
                ));
                endpoints.push(endpoint_for(
                    process_info,
                    AgyScheme::Http,
                    port,
                    Some(csrf_token.clone()),
                    true,
                    true,
                ));
            }
        }

        if let Some(extension_port) = process_info.extension_port {
            if listening_ports.contains(&extension_port) {
                let token = process_info
                    .extension_server_csrf_token
                    .clone()
                    .or_else(|| process_info.csrf_token.clone());
                if let Some(token) = token {
                    endpoints.push(endpoint_for(
                        process_info,
                        AgyScheme::Http,
                        extension_port,
                        Some(token),
                        true,
                        true,
                    ));
                }
            }
        }
    }

    if endpoints.is_empty() {
        if let Some(error) = discovery_error {
            return Err(error);
        }
    }
    endpoints.sort_by(compare_endpoints);
    Ok(endpoints)
}

#[allow(clippy::too_many_arguments)]
fn endpoint_for(
    process_info: &AgyProcessInfo,
    scheme: AgyScheme,
    port: u32,
    csrf_token: Option<String>,
    requires_csrf_token: bool,
    requires_unleash_probe: bool,
) -> AgyConnectionEndpoint {
    AgyConnectionEndpoint {
        scheme,
        port,
        source: process_info.source,
        pid: process_info.pid,
        csrf_token,
        requires_csrf_token,
        requires_unleash_probe,
    }
}

fn fetch_endpoint_quota(
    runtime: &dyn AgyProbeRuntime,
    endpoint: &AgyConnectionEndpoint,
    deadline: Instant,
) -> Result<QuotaResult, AgyError> {
    let mut summary_error: Option<AgyError> = None;
    match within_probe_budget(deadline, REQUEST_TIMEOUT_MS, |timeout_ms| {
        runtime.request_json(endpoint, QUOTA_SUMMARY_PATH, timeout_ms)
    }) {
        Ok(raw) => match normalize_agy_quota_summary(&raw) {
            Some(summary) => {
                let identity = fetch_endpoint_identity(runtime, endpoint, deadline);
                return Ok(QuotaResult {
                    plan: identity.as_ref().and_then(|i| i.plan.clone()),
                    account: identity.and_then(|i| i.account),
                    windows: summary.windows,
                    refreshed_at: summary.refreshed_at,
                });
            }
            None => {
                summary_error =
                    Some(AgyError::Malformed("Antigravity quota summary malformed".to_string()));
            }
        },
        Err(error) => {
            summary_error = stronger_failure(summary_error, error);
        }
    }

    for path in [USER_STATUS_PATH, COMMAND_MODEL_CONFIGS_PATH] {
        match within_probe_budget(deadline, REQUEST_TIMEOUT_MS, |timeout_ms| {
            runtime.request_json(endpoint, path, timeout_ms)
        }) {
            Ok(raw) => {
                if let Some(fallback) = normalize_agy_user_status(&raw) {
                    if !fallback.windows.is_empty() {
                        return Ok(QuotaResult {
                            plan: fallback.plan,
                            account: fallback.account,
                            windows: fallback.windows,
                            refreshed_at: fallback.refreshed_at,
                        });
                    }
                }
            }
            Err(error) => {
                summary_error = stronger_failure(summary_error, error);
            }
        }
    }

    match summary_error {
        Some(error) => Err(error),
        None => Err(AgyError::Other("Antigravity quota unavailable".to_string())),
    }
}

fn probe_endpoint(
    runtime: &dyn AgyProbeRuntime,
    endpoint: &AgyConnectionEndpoint,
    deadline: Instant,
) -> Result<(), AgyError> {
    within_probe_budget(deadline, REQUEST_TIMEOUT_MS, |timeout_ms| {
        runtime
            .request_json(endpoint, UNLEASH_PATH, timeout_ms)
            .map(|_| ())
    })
}

fn fetch_endpoint_identity(
    runtime: &dyn AgyProbeRuntime,
    endpoint: &AgyConnectionEndpoint,
    deadline: Instant,
) -> Option<IdentityResult> {
    match within_probe_budget(deadline, REQUEST_TIMEOUT_MS, |timeout_ms| {
        runtime.request_json(endpoint, USER_STATUS_PATH, timeout_ms)
    }) {
        Ok(raw) => normalize_agy_user_status(&raw).map(|status| IdentityResult {
            plan: status.plan,
            account: status.account,
        }),
        Err(_) => None,
    }
}

fn read_process_list(
    runtime: &dyn AgyProbeRuntime,
    deadline: Instant,
) -> Result<String, AgyError> {
    if cfg!(windows) {
        return Ok(String::new());
    }
    let Some(uid) = effective_uid() else {
        return Err(AgyError::Discovery("Antigravity process discovery failed".to_string()));
    };
    within_probe_budget(deadline, PROCESS_TIMEOUT_MS, |timeout_ms| {
        runtime
            .exec_file_text("ps", &current_user_process_list_args(uid), timeout_ms)
            .map_err(|_| AgyError::Discovery("Antigravity process discovery failed".to_string()))
    })
}

fn read_listening_ports(
    runtime: &dyn AgyProbeRuntime,
    pid: u32,
    deadline: Instant,
) -> Result<Vec<u32>, AgyError> {
    if cfg!(windows) {
        return Ok(Vec::new());
    }
    let args = vec![
        "-nP".to_string(),
        "-a".to_string(),
        "-p".to_string(),
        pid.to_string(),
        "-iTCP".to_string(),
        "-sTCP:LISTEN".to_string(),
    ];
    within_probe_budget(deadline, PORT_TIMEOUT_MS, |timeout_ms| {
        runtime
            .exec_file_text("lsof", &args, timeout_ms)
            .map(|output| ports_from_lsof(&output))
            .map_err(|_| AgyError::Discovery("Antigravity port discovery failed".to_string()))
    })
}

fn normalize_quota_summary_group(raw: &Value) -> Vec<QuotaWindow> {
    let Some(group) = raw.as_object() else {
        return Vec::new();
    };
    let group_name = string_value(group.get("displayName")).unwrap_or_else(|| "Quota".to_string());
    array_value(group.get("buckets"))
        .iter()
        .filter_map(|bucket| normalize_quota_summary_bucket(&group_name, bucket))
        .collect()
}

fn normalize_quota_summary_bucket(group_name: &str, raw: &Value) -> Option<QuotaWindow> {
    let bucket = raw.as_object()?;
    if boolean_value(bucket.get("disabled")).unwrap_or(false) {
        return None;
    }
    let bucket_id = string_value(bucket.get("bucketId"))
        .or_else(|| string_value(bucket.get("bucket_id")))?;
    let window_kind = agy_window_kind(bucket);
    let group = agy_window_group(group_name, &bucket_id);
    let label = format!("{} {}", group.label, window_kind.label);
    let mut result = QuotaWindow {
        id: format!("{}_{}", group.id, window_kind.id),
        label,
        kind: window_kind.kind,
        percent_used: None,
        percent_remaining: None,
        starts_at: None,
        resets_at: bucket
            .get("resetTime")
            .and_then(parse_epoch_or_iso)
            .or_else(|| bucket.get("reset_time").and_then(parse_epoch_or_iso)),
        reset_text: string_value(bucket.get("description")),
        window_seconds: None,
        spent_usd: None,
        limit_usd: None,
        pace: None,
    };
    if let Some(remaining) = remaining_fraction(bucket) {
        let percent_used = clamp_percent((1.0 - clamp_fraction(remaining)) * 100.0);
        result.percent_used = Some(percent_used);
        result.percent_remaining = percent_remaining(Some(percent_used));
    }
    if result.percent_used.is_none() && result.resets_at.is_none() && result.reset_text.is_none() {
        return None;
    }
    Some(result)
}

fn normalize_model_config_windows(data: &Map<String, Value>) -> Vec<QuotaWindow> {
    array_value(data.get("clientModelConfigs"))
        .iter()
        .filter_map(normalize_model_config_window)
        .collect()
}

fn normalize_model_config_window(raw: &Value) -> Option<QuotaWindow> {
    let config = raw.as_object()?;
    let label = string_value(config.get("label"))?;
    let quota_info = object_value(config.get("quotaInfo"))?;
    let model_or_alias = object_value(config.get("modelOrAlias"));
    let model_id = model_or_alias
        .and_then(|m| string_value(m.get("model")))
        .or_else(|| model_or_alias.and_then(|m| string_value(m.get("alias"))))
        .unwrap_or_else(|| slugify(&label));
    if model_id.is_empty() {
        return None;
    }
    let mut result = QuotaWindow {
        id: format!("model:{}", slugify(&model_id)),
        label,
        kind: WindowKind::Model,
        percent_used: None,
        percent_remaining: None,
        starts_at: None,
        resets_at: quota_info
            .get("resetTime")
            .and_then(parse_epoch_or_iso)
            .or_else(|| quota_info.get("reset_time").and_then(parse_epoch_or_iso)),
        reset_text: None,
        window_seconds: None,
        spent_usd: None,
        limit_usd: None,
        pace: None,
    };
    if let Some(remaining) = remaining_fraction(quota_info) {
        let percent_used = clamp_percent((1.0 - clamp_fraction(remaining)) * 100.0);
        result.percent_used = Some(percent_used);
        result.percent_remaining = percent_remaining(Some(percent_used));
    }
    if result.percent_used.is_none() && result.resets_at.is_none() {
        return None;
    }
    Some(result)
}

fn quota_summary_payload(raw: &Value) -> Option<&Map<String, Value>> {
    let data = raw.as_object()?;
    if let Some(response) = object_value(data.get("response")) {
        return Some(response);
    }
    if let Some(summary) = object_value(data.get("summary")) {
        return Some(summary);
    }
    if data.get("groups").map(Value::is_array).unwrap_or(false) {
        return Some(data);
    }
    None
}

fn agy_window_group(group_name: &str, bucket_id: &str) -> WindowGroup {
    let normalized = format!("{group_name} {bucket_id}").to_lowercase();
    if normalized.contains("gemini") {
        return WindowGroup {
            id: "gemini".to_string(),
            label: "Gemini".to_string(),
        };
    }
    if normalized.contains("claude") || normalized.contains("gpt") || normalized.contains("3p") {
        return WindowGroup {
            id: "claude_gpt".to_string(),
            label: "Claude/GPT".to_string(),
        };
    }
    let id = slugify(group_name);
    let id = if id.is_empty() { "quota".to_string() } else { id };
    WindowGroup {
        id,
        label: group_name.to_string(),
    }
}

fn agy_window_kind(bucket: &Map<String, Value>) -> WindowKindInfo {
    let raw = ["window", "bucketId", "bucket_id", "displayName"]
        .iter()
        .filter_map(|key| string_value(bucket.get(*key)))
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase();
    if raw.contains("5h") || raw.contains("five") {
        return WindowKindInfo {
            id: "5h".to_string(),
            label: "5-hour".to_string(),
            kind: WindowKind::Session,
        };
    }
    if raw.contains("week") {
        return WindowKindInfo {
            id: "weekly".to_string(),
            label: "weekly".to_string(),
            kind: WindowKind::Weekly,
        };
    }
    WindowKindInfo {
        id: "unknown".to_string(),
        label: "quota".to_string(),
        kind: WindowKind::Unknown,
    }
}

fn remaining_fraction(data: &Map<String, Value>) -> Option<f64> {
    number_value(data.get("remainingFraction"))
        .or_else(|| number_value(data.get("remaining_fraction")))
        .or_else(|| oneof_remaining_fraction(data.get("remaining")))
}

fn oneof_remaining_fraction(raw: Option<&Value>) -> Option<f64> {
    let direct = number_value(raw);
    if direct.is_some() {
        return direct;
    }
    let data = raw.and_then(Value::as_object)?;
    if let Some(value) = number_value(data.get("remainingFraction")) {
        return Some(value);
    }
    if let Some(value) = number_value(data.get("remaining_fraction")) {
        return Some(value);
    }
    let case = string_value(data.get("case"));
    if case.as_deref() == Some("remainingFraction") || case.as_deref() == Some("remaining_fraction")
    {
        return number_value(data.get("value"));
    }
    None
}

fn compare_agy_windows(left: &QuotaWindow, right: &QuotaWindow) -> std::cmp::Ordering {
    let left_group = window_group_rank(&left.id);
    let right_group = window_group_rank(&right.id);
    if left_group != right_group {
        return left_group.cmp(&right_group);
    }
    window_kind_rank(left).cmp(&window_kind_rank(right))
}

fn window_group_rank(id: &str) -> u8 {
    if id.starts_with("gemini_") {
        0
    } else if id.starts_with("claude_gpt_") {
        1
    } else {
        2
    }
}

fn window_kind_rank(window: &QuotaWindow) -> u8 {
    match window.kind {
        WindowKind::Session => 0,
        WindowKind::Weekly => 1,
        _ => 2,
    }
}

fn agy_process_source(command: &str) -> Option<AgyProcessSource> {
    let command_tokens = tokenize_command(command);
    let invoked_executable = executable_name(command_tokens.first().map(String::as_str).unwrap_or(""));
    if invoked_executable.as_deref() == Some("agy") {
        return Some(AgyProcessSource::Agy);
    }

    if let Some(exe) = invoked_executable.as_deref() {
        if is_node_family(exe) {
            let script = if exe.starts_with("mcp-server") {
                command_tokens.first().map(String::as_str)
            } else {
                command_tokens.get(1).map(String::as_str)
            };
            if is_agy_mcp_script(script) {
                return Some(AgyProcessSource::Agy);
            }
        }
    }

    if is_agy_app_executable(command) {
        return Some(AgyProcessSource::App);
    }
    None
}

fn tokenize_command(command: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let chars: Vec<char> = command.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c.is_whitespace() {
            i += 1;
            continue;
        }
        if c == '"' {
            i += 1;
            let mut s = String::new();
            while i < chars.len() && chars[i] != '"' {
                s.push(chars[i]);
                i += 1;
            }
            if i < chars.len() {
                i += 1; // closing quote
            }
            tokens.push(s);
        } else if c == '\'' {
            i += 1;
            let mut s = String::new();
            while i < chars.len() && chars[i] != '\'' {
                s.push(chars[i]);
                i += 1;
            }
            if i < chars.len() {
                i += 1;
            }
            tokens.push(s);
        } else {
            let mut s = String::new();
            while i < chars.len() && !chars[i].is_whitespace() {
                s.push(chars[i]);
                i += 1;
            }
            tokens.push(s);
        }
    }
    tokens
}

fn is_node_family(exe: &str) -> bool {
    matches!(exe, "node" | "nodejs" | "bun" | "mcp-server" | "mcp-server.cjs")
}

fn is_agy_mcp_script(token: Option<&str>) -> bool {
    let Some(token) = token else {
        return false;
    };
    let path = normalized_path(token);
    let segments: Vec<&str> = path.split('/').collect();
    segments.last() == Some(&"mcp-server.cjs") && segments.contains(&"antigravity-cli")
}

fn is_agy_app_executable(command: &str) -> bool {
    let normalized = normalized_path(command);
    if matches_app_language_server_command(&normalized) {
        return true;
    }
    let Some(first_token) = tokenize_command(command).into_iter().next() else {
        return false;
    };
    let path = normalized_path(&first_token);
    path.contains("antigravity") && matches_language_server_basename(&path)
}

fn matches_app_language_server_command(normalized: &str) -> bool {
    // ^/applications/[^\n]*antigravity\.app/contents/[^\n]*/language[-_]server(?:_[a-z0-9_]+)?(?=\s+--|$)
    if !normalized.starts_with("/applications/") {
        return false;
    }
    let Some(agy_app_idx) = normalized.find("antigravity.app") else {
        return false;
    };
    let after_agy_app = &normalized[agy_app_idx + "antigravity.app".len()..];
    let Some(contents_idx) = after_agy_app.find("/contents/") else {
        return false;
    };
    let after_contents = &after_agy_app[contents_idx + "/contents/".len()..];
    let Some(ls_idx) = after_contents
        .find("/language-server")
        .or_else(|| after_contents.find("/language_server"))
    else {
        return false;
    };
    let mut rest = &after_contents[ls_idx + "/language-server".len()..];
    if let Some(stripped) = rest.strip_prefix('_') {
        let suffix_len = stripped
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
            .count();
        if suffix_len == 0 {
            return false;
        }
        rest = &stripped[suffix_len..];
    }
    if rest.is_empty() {
        return true;
    }
    if rest.chars().next().map(|c| c.is_whitespace()).unwrap_or(false) {
        return rest.trim_start().starts_with("--");
    }
    false
}

fn matches_language_server_basename(path: &str) -> bool {
    // (?:^|\/)language[-_]server(?:_[a-z0-9_]+)?(?:\.exe)?$
    let basename_start = match path.rfind('/') {
        Some(idx) => idx + 1,
        None => 0,
    };
    let basename = &path[basename_start..];
    let rest = if let Some(r) = basename.strip_prefix("language-server") {
        r
    } else if let Some(r) = basename.strip_prefix("language_server") {
        r
    } else {
        return false;
    };
    let rest = if let Some(stripped) = rest.strip_prefix('_') {
        let suffix_len = stripped
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
            .count();
        if suffix_len == 0 {
            return false;
        }
        &stripped[suffix_len..]
    } else {
        rest
    };
    let rest = rest.strip_suffix(".exe").unwrap_or(rest);
    rest.is_empty()
}

fn normalized_path(value: &str) -> String {
    value.replace('\\', "/").to_lowercase()
}

fn executable_name(command: &str) -> Option<String> {
    let token = command.trim().split_whitespace().next()?;
    let name = token.replace('\\', "/");
    let name = name.split('/').next_back()?.to_lowercase();
    Some(name.strip_suffix(".exe").unwrap_or(&name).to_string())
}

fn flag_value(command: &str, name: &str) -> Option<String> {
    let needle = format!("--{name}");
    let mut search_from = 0;
    while let Some(pos) = command[search_from..].find(&needle) {
        let i = search_from + pos;
        let preceded_ok = i == 0
            || command[..i]
                .chars()
                .last()
                .map(|c| c.is_whitespace())
                .unwrap_or(false);
        if preceded_ok {
            let after = &command[i + needle.len()..];
            if let Some(rest) = after.strip_prefix('=') {
                let value = rest.split_whitespace().next().unwrap_or("");
                if !value.is_empty() {
                    return Some(value.to_string());
                }
            } else if after.chars().next().map(|c| c.is_whitespace()).unwrap_or(false) {
                let value = after.trim_start().split_whitespace().next().unwrap_or("");
                if !value.is_empty() {
                    return Some(value.to_string());
                }
            }
        }
        search_from = i + needle.len();
    }
    None
}

fn compare_endpoints(
    left: &AgyConnectionEndpoint,
    right: &AgyConnectionEndpoint,
) -> std::cmp::Ordering {
    let source_rank = source_sort_rank(left.source).cmp(&source_sort_rank(right.source));
    if source_rank != std::cmp::Ordering::Equal {
        return source_rank;
    }
    let port_rank = left.port.cmp(&right.port);
    if port_rank != std::cmp::Ordering::Equal {
        return port_rank;
    }
    scheme_sort_rank(left.scheme).cmp(&scheme_sort_rank(right.scheme))
}

fn source_sort_rank(source: AgyProcessSource) -> u8 {
    match source {
        AgyProcessSource::Agy => 0,
        AgyProcessSource::App => 1,
    }
}

fn scheme_sort_rank(scheme: AgyScheme) -> u8 {
    match scheme {
        AgyScheme::Https => 0,
        AgyScheme::Http => 1,
    }
}

fn status_for_error(error: &str) -> ProviderStatus {
    let lower = error.to_lowercase();
    if lower.contains("not running")
        || lower.contains("no local")
        || lower.contains("loopback unavailable")
        || lower.contains("loopback timed out")
        || lower.contains("probe timed out")
        || lower.contains("econnrefused")
        || lower.contains("econnreset")
        || lower.contains("econnaborted")
        || lower.contains("enotfound")
        || lower.contains("ehostunreach")
        || lower.contains("etimedout")
        || lower.contains("epipe")
        || lower.contains("eproto")
        || lower.contains("socket hang up")
    {
        return ProviderStatus::Unavailable;
    }
    status_from_error(error)
}

pub fn request_loopback_json(
    endpoint: &AgyConnectionEndpoint,
    path: &str,
    timeout_ms: u64,
) -> Result<Value, AgyError> {
    let body = match serde_json::to_string(&request_body_for_path(path)) {
        Ok(body) => body,
        Err(_) => {
            return Err(AgyError::Malformed(
                "Antigravity loopback returned invalid JSON".to_string(),
            ))
        }
    };
    let scheme = match endpoint.scheme {
        AgyScheme::Https => "https",
        AgyScheme::Http => "http",
    };
    let url = format!("{scheme}://127.0.0.1:{}{path}", endpoint.port);

    let mut builder = reqwest::blocking::Client::builder()
        .timeout(Duration::from_millis(timeout_ms));
    if endpoint.scheme == AgyScheme::Https {
        builder = builder.danger_accept_invalid_certs(true);
    }
    let client = match builder.build() {
        Ok(client) => client,
        Err(error) => return Err(sanitize_transport_error(&error)),
    };

    let mut request = client
        .post(url.as_str())
        .header("accept", "application/json")
        .header("content-type", "application/json")
        .header("content-length", body.len().to_string())
        .header("connect-protocol-version", "1");
    if endpoint.requires_csrf_token {
        if let Some(token) = &endpoint.csrf_token {
            request = request.header("x-codeium-csrf-token", token);
        }
    }

    let response = match request.body(body).send() {
        Ok(response) => response,
        Err(error) => {
            if error.is_timeout() {
                return Err(AgyError::Unavailable("Antigravity loopback timed out".to_string()));
            }
            return Err(sanitize_transport_error(&error));
        }
    };

    let status = response.status().as_u16() as u32;
    if status < 200 || status >= 300 {
        return Err(AgyError::Http(status));
    }

    let mut limited = response.take((MAX_RESPONSE_BYTES + 1) as u64);
    let mut data = Vec::new();
    if let Err(error) = limited.read_to_end(&mut data) {
        if error.kind() == std::io::ErrorKind::TimedOut {
            return Err(AgyError::Unavailable("Antigravity loopback timed out".to_string()));
        }
        return Err(sanitize_io_error(&error));
    }
    if data.len() > MAX_RESPONSE_BYTES {
        return Err(AgyError::Malformed(
            "Antigravity loopback response too large".to_string(),
        ));
    }
    let text = match String::from_utf8(data) {
        Ok(text) => text,
        Err(_) => {
            return Err(AgyError::Malformed(
                "Antigravity loopback returned invalid JSON".to_string(),
            ))
        }
    };
    match serde_json::from_str::<Value>(&text) {
        Ok(value) => Ok(value),
        Err(_) => Err(AgyError::Malformed(
            "Antigravity loopback returned invalid JSON".to_string(),
        )),
    }
}

fn request_body_for_path(path: &str) -> Value {
    if path == QUOTA_SUMMARY_PATH {
        serde_json::json!({ "forceRefresh": false })
    } else {
        serde_json::json!({
            "metadata": {
                "ideName": "antigravity",
                "extensionName": "antigravity",
                "ideVersion": "unknown",
                "locale": "en",
            }
        })
    }
}

fn clamp_fraction(value: f64) -> f64 {
    if !value.is_finite() {
        return 0.0;
    }
    value.clamp(0.0, 1.0)
}

fn object_value(value: Option<&Value>) -> Option<&Map<String, Value>> {
    value?.as_object()
}

fn array_value(value: Option<&Value>) -> &[Value] {
    match value.and_then(Value::as_array) {
        Some(arr) => arr.as_slice(),
        None => &[],
    }
}

fn string_value(value: Option<&Value>) -> Option<String> {
    match value?.as_str() {
        Some(s) if !s.is_empty() => Some(s.to_string()),
        _ => None,
    }
}

fn number_value(value: Option<&Value>) -> Option<f64> {
    match value? {
        Value::Number(n) => n.as_f64().filter(|f| f.is_finite()),
        Value::String(s) => {
            let trimmed = s.trim();
            if trimmed.is_empty() {
                return None;
            }
            trimmed.parse::<f64>().ok().filter(|f| f.is_finite())
        }
        _ => None,
    }
}

fn boolean_value(value: Option<&Value>) -> Option<bool> {
    value?.as_bool()
}

fn slugify(value: &str) -> String {
    let lower = value.trim().to_lowercase();
    let mut out = String::new();
    let mut last_was_sep = true;
    for c in lower.chars() {
        if c.is_ascii_lowercase() || c.is_ascii_digit() {
            out.push(c);
            last_was_sep = false;
        } else if !last_was_sep {
            out.push('_');
            last_was_sep = true;
        }
    }
    while out.ends_with('_') {
        out.pop();
    }
    out
}

fn stronger_failure(current: Option<AgyError>, candidate: AgyError) -> Option<AgyError> {
    match current {
        None => Some(candidate),
        Some(current) => {
            if failure_rank(&candidate) > failure_rank(&current) {
                Some(candidate)
            } else {
                Some(current)
            }
        }
    }
}

fn failure_rank(error: &AgyError) -> u8 {
    match status_for_error(&error.message()) {
        ProviderStatus::AuthRequired => 4,
        ProviderStatus::RateLimited => 3,
        ProviderStatus::Error => 2,
        ProviderStatus::Unavailable => 1,
        _ => 0,
    }
}

fn stale_eligible_failure(error: &AgyError) -> bool {
    if let AgyError::Http(status) = error {
        return *status == 429 || *status >= 500;
    }
    status_for_error(&error.message()) == ProviderStatus::Unavailable
}

fn is_definitive_auth_failure(error: &AgyError) -> bool {
    status_for_error(&error.message()) == ProviderStatus::AuthRequired
}

fn create_probe_deadline() -> Instant {
    Instant::now() + Duration::from_millis(PROBE_BUDGET_MS)
}

fn within_probe_budget<T>(
    deadline: Instant,
    per_operation_timeout_ms: u64,
    operation: impl FnOnce(u64) -> Result<T, AgyError>,
) -> Result<T, AgyError> {
    let remaining_ms = deadline
        .saturating_duration_since(Instant::now())
        .as_millis() as u64;
    if remaining_ms == 0 {
        return Err(AgyError::ProbeBudget);
    }
    let timeout_ms = per_operation_timeout_ms.min(remaining_ms);
    operation(timeout_ms)
}

fn http_error_message(status: u32) -> String {
    if status == 401 || status == 403 {
        "Antigravity sign-in required".to_string()
    } else if status == 429 {
        "Antigravity quota endpoint rate limited".to_string()
    } else {
        format!("Antigravity quota endpoint returned HTTP {status}")
    }
}

fn sanitize_transport_error(error: &reqwest::Error) -> AgyError {
    if let Some(code) = transport_error_code(error) {
        return AgyError::Unavailable(format!("Antigravity loopback unavailable ({code})"));
    }
    AgyError::Unavailable("Antigravity loopback unavailable".to_string())
}

fn sanitize_io_error(error: &std::io::Error) -> AgyError {
    if let Some(code) = io_error_code_name(error.kind()) {
        return AgyError::Unavailable(format!("Antigravity loopback unavailable ({code})"));
    }
    AgyError::Unavailable("Antigravity loopback unavailable".to_string())
}

fn transport_error_code(error: &(dyn std::error::Error + 'static)) -> Option<String> {
    let mut current: Option<&(dyn std::error::Error + 'static)> = Some(error);
    while let Some(err) = current {
        if let Some(io) = err.downcast_ref::<std::io::Error>() {
            return io_error_code_name(io.kind());
        }
        current = err.source();
    }
    None
}

fn io_error_code_name(kind: std::io::ErrorKind) -> Option<String> {
    use std::io::ErrorKind::*;
    Some(match kind {
        ConnectionRefused => "ECONNREFUSED",
        ConnectionReset => "ECONNRESET",
        ConnectionAborted => "ECONNABORTED",
        TimedOut => "ETIMEDOUT",
        NotFound => "ENOTFOUND",
        BrokenPipe => "EPIPE",
        HostUnreachable => "EHOSTUNREACH",
        NetworkUnreachable => "ENETUNREACH",
        _ => return None,
    }
    .to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(s: &str) -> Value {
        serde_json::from_str(s).expect("valid JSON fixture")
    }

    const QUOTA_SUMMARY: &str = r#"{
  "response": {
    "groups": [
      {
        "displayName": "Gemini Models",
        "description": "Models within this group: Gemini Flash, Gemini Pro",
        "buckets": [
          {
            "bucketId": "gemini-weekly",
            "displayName": "Weekly Limit",
            "description": "Fixture weekly reset",
            "window": "weekly",
            "remainingFraction": 0.82,
            "resetTime": "2026-06-19T08:45:39Z"
          },
          {
            "bucketId": "gemini-5h",
            "displayName": "Five Hour Limit",
            "window": "5h",
            "remainingFraction": 0.91,
            "resetTime": "2026-06-15T11:39:34Z"
          }
        ]
      },
      {
        "displayName": "Claude and GPT models",
        "description": "Models within this group: Claude Opus, Claude Sonnet, GPT-OSS",
        "buckets": [
          {
            "bucketId": "3p-weekly",
            "displayName": "Weekly Limit",
            "description": "Fixture third-party weekly reset",
            "window": "weekly",
            "remainingFraction": 0.64,
            "resetTime": "2026-06-20T00:39:54Z"
          },
          {
            "bucketId": "3p-5h",
            "displayName": "Five Hour Limit",
            "window": "5h",
            "remainingFraction": 0.73,
            "resetTime": "2026-06-15T12:52:10Z"
          }
        ]
      }
    ],
    "description": "Fixture quota summary"
  }
}"#;

    const USER_STATUS: &str = r#"{
  "userStatus": {
    "email": "person@example.invalid",
    "planStatus": {
      "planInfo": {
        "planName": "Pro"
      }
    },
    "cascadeModelConfigData": {
      "clientModelConfigs": [
        {
          "label": "Gemini 3.5 Flash (Medium)",
          "modelOrAlias": {
            "model": "MODEL_FIXTURE_GEMINI_FLASH"
          },
          "quotaInfo": {
            "remainingFraction": 1,
            "resetTime": "2026-06-15T12:52:10Z"
          }
        },
        {
          "label": "Claude Sonnet Fixture",
          "modelOrAlias": {
            "model": "MODEL_FIXTURE_CLAUDE_SONNET"
          },
          "quotaInfo": {
            "remainingFraction": 0.5,
            "resetTime": "2026-06-15T12:52:10Z"
          }
        }
      ]
    },
    "userTier": {
      "name": "Google AI Pro"
    }
  }
}"#;

    #[test]
    fn normalizes_quota_summary_groups_into_session_and_weekly_windows() {
        let result = normalize_agy_quota_summary(&parse(QUOTA_SUMMARY)).expect("windows");
        let expected = [
            (
                "gemini_5h",
                "Gemini 5-hour",
                WindowKind::Session,
                9.0,
                91.0,
                "2026-06-15T11:39:34.000Z",
            ),
            (
                "gemini_weekly",
                "Gemini weekly",
                WindowKind::Weekly,
                18.0,
                82.0,
                "2026-06-19T08:45:39.000Z",
            ),
            (
                "claude_gpt_5h",
                "Claude/GPT 5-hour",
                WindowKind::Session,
                27.0,
                73.0,
                "2026-06-15T12:52:10.000Z",
            ),
            (
                "claude_gpt_weekly",
                "Claude/GPT weekly",
                WindowKind::Weekly,
                36.0,
                64.0,
                "2026-06-20T00:39:54.000Z",
            ),
        ];
        assert_eq!(result.windows.len(), expected.len());
        for (window, (id, label, kind, used, remaining, resets)) in
            result.windows.iter().zip(expected)
        {
            assert_eq!(window.id, id);
            assert_eq!(window.label, label);
            assert_eq!(window.kind, kind);
            assert_eq!(window.percent_used, Some(used));
            assert_eq!(window.percent_remaining, Some(remaining));
            assert_eq!(window.resets_at.as_deref(), Some(resets));
            assert_eq!(window.window_seconds, None);
        }
    }

    #[test]
    fn normalizes_oneof_remaining_values() {
        let raw = parse(
            r#"{ "groups": [ { "displayName": "Gemini Models", "buckets": [ { "bucketId": "gemini-weekly", "displayName": "Weekly Limit", "remaining": { "case": "remainingFraction", "value": 0.5 } } ] } ] }"#,
        );
        let result = normalize_agy_quota_summary(&raw).expect("windows");
        let window = &result.windows[0];
        assert_eq!(window.id, "gemini_weekly");
        assert_eq!(window.percent_used, Some(50.0));
        assert_eq!(window.percent_remaining, Some(50.0));
        assert_eq!(window.window_seconds, None);
    }

    #[test]
    fn falls_back_to_model_windows_from_user_status() {
        let result = normalize_agy_user_status(&parse(USER_STATUS)).expect("status");
        assert_eq!(result.plan.as_deref(), Some("Google AI Pro"));
        assert_eq!(
            result
                .account
                .as_ref()
                .and_then(|a| a.email.as_deref()),
            Some("person@example.invalid")
        );
        assert_eq!(result.windows.len(), 2);
        assert_eq!(result.windows[0].id, "model:model_fixture_gemini_flash");
        assert_eq!(result.windows[0].kind, WindowKind::Model);
        assert_eq!(result.windows[0].percent_remaining, Some(100.0));
        assert_eq!(result.windows[1].id, "model:model_fixture_claude_sonnet");
        assert_eq!(result.windows[1].kind, WindowKind::Model);
        assert_eq!(result.windows[1].percent_remaining, Some(50.0));
    }

    #[test]
    fn parses_antigravity_processes_and_listening_ports_without_prompt_text() {
        let processes = process_infos_from_ps(
            "      101 /Users/test/.local/bin/agy\n\
             102 /Applications/Google Antigravity.app/Contents/Resources/bin/language-server --csrf_token token --extension_server_port 64123\n\
             103 /usr/bin/node /opt/antigravity-cli/mcp-server.cjs --port 64124\n\
             104 codex --prompt \"antigravity-cli mcp-server.cjs language_server\"\n\
             105 /usr/bin/node /opt/runner.cjs --prompt \"/opt/antigravity-cli/mcp-server.cjs\"\n\
             106 /usr/bin/codex --prompt \"/Applications/Antigravity.app/Contents/bin/language_server --csrf_token fake\"\n",
        );

        assert_eq!(processes.len(), 3);
        assert_eq!(processes[0].pid, 101);
        assert_eq!(processes[0].source, AgyProcessSource::Agy);

        assert_eq!(processes[1].pid, 102);
        assert_eq!(processes[1].source, AgyProcessSource::App);
        assert_eq!(processes[1].csrf_token.as_deref(), Some("token"));
        assert_eq!(processes[1].extension_port, Some(64123));

        assert_eq!(processes[2].pid, 103);
        assert_eq!(processes[2].source, AgyProcessSource::Agy);

        let ports = ports_from_lsof(
            "COMMAND PID USER FD TYPE DEVICE SIZE/OFF NODE NAME\n\
             agy 101 test 8u IPv4 0x1 0t0 TCP 127.0.0.1:64440 (LISTEN)\n\
             agy 101 test 9u IPv4 0x2 0t0 TCP 127.0.0.1:64441 (LISTEN)\n",
        );
        assert_eq!(ports, vec![64440, 64441]);
    }
}
