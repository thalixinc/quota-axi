//! Faithful Rust port of the `src/render.ts` quota path (epic #8).
//!
//! The default report is decision-shaped: one `quota[]` row per measurable
//! scope, plus sparse `exhaustion[]` and `attention[]` blocks. `--full` adds the
//! audit blocks, and `--json` demotes derivation inputs *here*, never at
//! computation, so the in-memory normalized model keeps every field.
//!
//! Only the quota surface is ported here; `render_auth_toon` and
//! `render_models_toon` land in a follow-on PR.

use serde::Serialize;

use crate::providers::usage_fetch_failure::is_usage_fetch_failure;
use crate::toon::encode_string;
use crate::types::{
    BoundConflict, EffectiveAvailability, ProviderQuota, ProviderStatus, QuotaAxiResponse,
    QuotaSemantics, QuotaWindow, RunwayStatus, SELECTION_SCALAR_KEY,
};

const UNKNOWN: &str = "unknown";
const NONE: &str = "none";
const ID_SEPARATOR: &str = " + ";
/// Kept out of `,` so a detail never forces TOON string quoting on its own.
const DETAIL_SEPARATOR: &str = " · ";
const DESCRIPTION: &str = "Report local agent-provider quota windows for routing-aware agents";
const FULL_TIER_HINT: &str =
    "Run `quota-axi --full` for windows, pace, reserve, and account evidence";

const QUOTA_FIELDS: &[&str] = &[
    "provider",
    "scope",
    "effectivePercentRemaining",
    "spendPriority",
    "runway",
    "confidence",
    "limitedBy",
    "resetsAt",
];
const EXHAUSTION_FIELDS: &[&str] = &[
    "provider",
    "scope",
    "usableRunwaySeconds",
    "projectedExhaustedAt",
    "limitingWindowId",
];
const ATTENTION_FIELDS: &[&str] = &["provider", "scope", "kind", "detail", "remedy"];
const PROVIDERS_FIELDS: &[&str] = &[
    "provider",
    "plan",
    "source",
    "status",
    "authStatus",
    "relationships",
    "refreshedAt",
];
const WINDOWS_FIELDS: &[&str] = &[
    "provider",
    "id",
    "label",
    "percentRemaining",
    "resetsAt",
    "pace",
    "reserve",
    "burnMultiple",
    "timeRemainingPercent",
    "elapsedPercent",
    "cycleSeconds",
    "projectedExhaustedAt",
    "confidence",
];
const SCOPE_AUDIT_FIELDS: &[&str] = &[
    "provider",
    "scope",
    "boundedBy",
    "relationships",
    "pace",
    "aheadWindowIds",
    "behindWindowIds",
    "onPaceWindowIds",
    "unknownWindowIds",
    "worstReserve",
    "worstReserveWindowId",
];
const ACCOUNTS_FIELDS: &[&str] = &[
    "provider",
    "email",
    "organization",
    "accountId",
    "identityStatus",
];
const ATTEMPTS_FIELDS: &[&str] = &["provider", "source", "status", "error"];

/// A single cell of a tabular TOON row. Strings quote only when unsafe (matching
/// `@toon-format/toon`), numbers render via the shortest-roundtrip representation.
enum Prim {
    Str(String),
    Num(f64),
}

impl Prim {
    fn encode(&self) -> String {
        match self {
            Prim::Str(s) => encode_string(s),
            Prim::Num(n) => fmt_f64(*n),
        }
    }
}

/// `@toon-format/toon`'s `String(number)`: shortest round-trip, no trailing `.0`
/// for integral doubles, and `-0` normalized to `0` (a different claim).
fn fmt_f64(value: f64) -> String {
    if value == 0.0 {
        "0".to_string()
    } else {
        value.to_string()
    }
}

/// The serialized string form of a `#[serde(rename_all = ...)]` enum variant.
fn enum_str<T: Serialize>(value: &T) -> String {
    match serde_json::to_value(value) {
        Ok(serde_json::Value::String(s)) => s,
        other => unreachable!("enum serialized as non-string: {:?}", other),
    }
}

fn opt_num(value: Option<f64>) -> Prim {
    match value {
        Some(n) => Prim::Num(n),
        None => Prim::Str(UNKNOWN.to_string()),
    }
}

fn opt_string(value: Option<String>, fallback: &str) -> Prim {
    match value {
        Some(s) => Prim::Str(s),
        None => Prim::Str(fallback.to_string()),
    }
}

struct Tabular {
    key: &'static str,
    fields: &'static [&'static str],
    rows: Vec<Vec<Prim>>,
}

/// `encode({ key: rows })` for a tabular (array-of-objects) block.
fn encode_tabular(block: &Tabular) -> String {
    if block.rows.is_empty() {
        return format!("{}[0]:", block.key);
    }
    let mut out = format!(
        "{}[{}]{{{}}}:",
        block.key,
        block.rows.len(),
        block.fields.join(",")
    );
    for row in &block.rows {
        out.push('\n');
        out.push_str("  ");
        out.push_str(&row.iter().map(|cell| cell.encode()).collect::<Vec<_>>().join(","));
    }
    out
}

/// `encode({ bin, description, generatedAt })` — the report header block.
fn encode_header(bin_path: &str, generated_at: &str) -> String {
    format!(
        "bin: {}\ndescription: {}\ngeneratedAt: {}",
        encode_string(&collapse_home(bin_path)),
        encode_string(DESCRIPTION),
        encode_string(generated_at)
    )
}

/// `renderHelp`: a prose list block (`help[N]:` then two-space-indented lines),
/// distinct from the inline/list forms in `src/toon.rs`.
fn render_help(lines: &[String]) -> String {
    let mut out = format!("help[{}]:", lines.len());
    for line in lines {
        out.push('\n');
        out.push_str("  ");
        out.push_str(line);
    }
    out
}

/// `quotaHelpLines`: the response's situational advice first, then the one
/// tier hint worth repeating on every invocation.
fn quota_help_lines(response: &QuotaAxiResponse) -> Vec<String> {
    let mut lines = response.help.clone().unwrap_or_default();
    lines.push(FULL_TIER_HINT.to_string());
    lines
}

/// Render the default decision-shaped report. Blocks are separated by a single
/// newline — never a blank line — matching `blocks.filter(Boolean).join("\n")`.
pub fn render_quota_toon(response: &QuotaAxiResponse, bin_path: &str, full: bool) -> String {
    let blocks = quota_blocks(response);
    let mut parts: Vec<String> = Vec::new();
    parts.push(encode_header(bin_path, &response.generated_at));
    parts.push(encode_tabular(&Tabular {
        key: "quota",
        fields: QUOTA_FIELDS,
        rows: blocks.quota,
    }));
    parts.push(encode_tabular(&Tabular {
        key: "exhaustion",
        fields: EXHAUSTION_FIELDS,
        rows: blocks.exhaustion,
    }));
    parts.push(encode_tabular(&Tabular {
        key: "attention",
        fields: ATTENTION_FIELDS,
        rows: blocks.attention,
    }));
    if full {
        parts.extend(audit_blocks(response));
    }
    parts.push(render_help(&quota_help_lines(response)));
    parts.join("\n")
}

/// The `--json` payload: redaction plus derivation-input demotion. Field names
/// are untouched — a demoted field is simply absent until `--full`.
pub fn quota_json_report(response: &QuotaAxiResponse, full: bool) -> serde_json::Value {
    let mut redacted = redacted_response(response, full);
    if !full {
        for provider in &mut redacted.providers {
            provider.label = None;
            provider.source = None;
            for window in &mut provider.windows {
                demote_window(window);
            }
            if let Some(semantics) = &mut provider.quota_semantics {
                demote_semantics(semantics);
            }
            provider.state.refreshed_at = None;
            provider.state.sources_tried = None;
        }
    }
    serde_json::to_value(&redacted).expect("response serializes")
}

/// Hides account identity and attempts unless `--full` is set.
pub fn redacted_response(response: &QuotaAxiResponse, full: bool) -> QuotaAxiResponse {
    if full {
        return response.clone();
    }
    let mut redacted = response.clone();
    for provider in &mut redacted.providers {
        provider.account = None;
        provider.attempts = None;
    }
    redacted
}

fn demote_window(window: &mut QuotaWindow) {
    window.percent_used = None;
    window.starts_at = None;
    window.window_seconds = None;
    if let Some(pace) = &mut window.pace {
        // Kept: `reason` is uncertainty, reserve and burn are tie-break
        // evidence. Dropped: the inputs those two are derived from.
        pace.time_remaining_percent = None;
        pace.elapsed_percent = None;
        pace.projected_exhausted_at = None;
        pace.projection_confidence = None;
        pace.cycle_basis = None;
        pace.cycle_seconds = None;
    }
}

fn demote_semantics(semantics: &mut QuotaSemantics) {
    semantics.description = None;
    for availability in &mut semantics.effective_availability {
        if let Some(pace) = &mut availability.pace {
            pace.behind_window_ids = None;
            pace.on_pace_window_ids = None;
        }
    }
}

struct ProviderBlocks {
    quota: Vec<Vec<Prim>>,
    exhaustion: Vec<Vec<Prim>>,
    attention: Vec<Vec<Prim>>,
}

/// Contract invariant: every requested provider appears at least once, in
/// `quota[]` or `attention[]` or both, and never in metric order.
fn quota_blocks(response: &QuotaAxiResponse) -> ProviderBlocks {
    let mut blocks = ProviderBlocks {
        quota: Vec::new(),
        exhaustion: Vec::new(),
        attention: Vec::new(),
    };
    for provider in &response.providers {
        let scopes = provider
            .quota_semantics
            .as_ref()
            .map(|s| s.effective_availability.as_slice())
            .unwrap_or(&[]);
        let mut scope_attention: Vec<Vec<Prim>> = Vec::new();
        let mut measured = false;

        for scope in scopes {
            if scope.effective_percent_remaining.is_none() {
                let kind = if scope.bound_conflict.is_some() {
                    "bound_conflict"
                } else {
                    "headroom_unknown"
                };
                let detail = match &scope.bound_conflict {
                    Some(conflict) => bound_conflict_detail(conflict),
                    None => unknown_headroom_detail(scope),
                };
                scope_attention.push(attention_row(provider, &scope.scope, kind, detail, NONE));
            } else {
                measured = true;
                blocks.quota.push(quota_row(provider, scope));
                if let Some(row) = exhaustion_row(provider, scope) {
                    blocks.exhaustion.push(row);
                }
                if let Some(blocked) = blocked_signals(scope) {
                    scope_attention.push(attention_row(
                        provider,
                        &scope.scope,
                        "unmeasurable",
                        blocked,
                        NONE,
                    ));
                }
            }
        }

        blocks
            .attention
            .extend(provider_attention(provider, measured, scope_attention.len()));
        blocks.attention.extend(scope_attention);
    }
    blocks
}

fn quota_row(provider: &ProviderQuota, scope: &EffectiveAvailability) -> Vec<Prim> {
    let runway = scope.runway.as_ref();
    let spend_priority = scope
        .selection
        .as_ref()
        .and_then(|selection| selection.spend_priority);
    vec![
        Prim::Str(provider.provider.as_str().to_string()),
        Prim::Str(scope.scope.clone()),
        Prim::Num(scope.effective_percent_remaining.unwrap()),
        // `unknown`, never `0`: `0` is exact utilization, a different claim.
        spend_priority
            .map(Prim::Num)
            .unwrap_or_else(|| Prim::Str(UNKNOWN.to_string())),
        Prim::Str(
            runway
                .map(|r| enum_str(&r.status))
                .unwrap_or_else(|| UNKNOWN.to_string()),
        ),
        Prim::Str(
            runway
                .and_then(|r| r.projection_confidence.as_ref())
                .map(enum_str)
                .unwrap_or_else(|| UNKNOWN.to_string()),
        ),
        Prim::Str(
            join_ids(scope.limiting_window_ids.as_deref().unwrap_or(&[]))
                .unwrap_or_else(|| UNKNOWN.to_string()),
        ),
        Prim::Str(binding_reset(&provider.windows, scope)),
    ]
}

fn exhaustion_row(provider: &ProviderQuota, scope: &EffectiveAvailability) -> Option<Vec<Prim>> {
    let runway = scope.runway.as_ref()?;
    if runway.status != RunwayStatus::ExhaustedNow
        && runway.status != RunwayStatus::ProjectedExhaustion
    {
        return None;
    }
    Some(vec![
        Prim::Str(provider.provider.as_str().to_string()),
        Prim::Str(scope.scope.clone()),
        runway
            .usable_runway_seconds
            .map(Prim::Num)
            .unwrap_or_else(|| Prim::Str(UNKNOWN.to_string())),
        Prim::Str(runway.projected_exhausted_at.clone().unwrap_or_else(|| UNKNOWN.to_string())),
        Prim::Str(runway.limiting_window_id.clone().unwrap_or_else(|| UNKNOWN.to_string())),
    ])
}

fn attention_row(
    provider: &ProviderQuota,
    scope: &str,
    kind: &str,
    detail: String,
    remedy: &str,
) -> Vec<Prim> {
    vec![
        Prim::Str(provider.provider.as_str().to_string()),
        Prim::Str(scope.to_string()),
        Prim::Str(kind.to_string()),
        Prim::Str(detail),
        Prim::Str(remedy.to_string()),
    ]
}

/// Provider-level facts. A provider with no `quota[]` row always ends up with at
/// least one row across this block and its scope rows, so it can never be
/// silently absent, and it always states its auth fact — positive included.
fn provider_attention(
    provider: &ProviderQuota,
    measured: bool,
    scope_rows: usize,
) -> Vec<Vec<Prim>> {
    let mut rows = provider_state_rows(provider, measured, scope_rows);
    rows.extend(degraded_source_rows(provider));
    rows
}

/// Degraded sources are appended, never counted: they name the provider but not
/// why a scope is missing, so they must not suppress the `no_quota` row.
fn degraded_source_rows(provider: &ProviderQuota) -> Vec<Vec<Prim>> {
    provider
        .state
        .degraded_sources
        .as_ref()
        .map(|sources| {
            sources
                .iter()
                .map(|degraded| {
                    let detail = match &degraded.error {
                        Some(error) => format!("{}{}{}", degraded.source, DETAIL_SEPARATOR, error),
                        None => degraded.source.clone(),
                    };
                    attention_row(provider, "all", "degraded_source", detail, NONE)
                })
                .collect()
        })
        .unwrap_or_default()
}

fn provider_state_rows(
    provider: &ProviderQuota,
    measured: bool,
    scope_rows: usize,
) -> Vec<Vec<Prim>> {
    let mut rows: Vec<Vec<Prim>> = Vec::new();

    let primary = primary_provider_row(provider);
    let has_primary = primary.is_some();
    if let Some(row) = primary {
        rows.push(row);
    }

    if let Some(unresolved) = join_ids(
        provider
            .quota_semantics
            .as_ref()
            .and_then(|s| s.unresolved_window_ids.as_deref())
            .unwrap_or(&[]),
    ) {
        rows.push(attention_row(
            provider,
            "all",
            "unresolved_windows",
            unresolved,
            NONE,
        ));
    }
    if let Some(untrusted) = join_ids(provider.state.untrusted_window_ids.as_deref().unwrap_or(&[]))
    {
        rows.push(attention_row(
            provider,
            "all",
            "untrusted_windows",
            untrusted,
            NONE,
        ));
    }

    if measured {
        return rows;
    }

    let suffix = provider
        .state
        .auth_status
        .as_ref()
        .map(|status| format!(" (auth {})", enum_str(status)))
        .unwrap_or_default();

    // A status row carries the auth fact in its detail (`primary.detail += suffix`).
    if has_primary {
        if let Prim::Str(detail) = &mut rows[0][3] {
            detail.push_str(&suffix);
        }
        return rows;
    }

    // No status row to carry the auth fact. Emit one when there is an auth
    // status to state, or when nothing else would name this provider at all.
    if suffix.is_empty() && rows.len() + scope_rows > 0 {
        return rows;
    }
    let detail = format!(
        "{}{}",
        provider
            .state
            .error
            .clone()
            .unwrap_or_else(|| "no measurable scope".to_string()),
        suffix
    );
    rows.insert(
        0,
        attention_row(
            provider,
            "all",
            "no_quota",
            detail,
            provider.state.remedy_command.as_deref().unwrap_or(NONE),
        ),
    );
    rows
}

fn primary_provider_row(provider: &ProviderQuota) -> Option<Vec<Prim>> {
    let state = &provider.state;
    if !state.stale && state.status == ProviderStatus::Fresh {
        return None;
    }
    let kind = if state.stale {
        "stale".to_string()
    } else {
        enum_str(&state.status)
    };

    let stale_detail = {
        let mut parts: Vec<String> = Vec::new();
        parts.push(format!(
            "last refreshed {}",
            state.refreshed_at.as_deref().unwrap_or(UNKNOWN)
        ));
        if let Some(error) = &state.error {
            let prefix = if is_usage_fetch_failure(provider) {
                "fetch failed "
            } else {
                ""
            };
            parts.push(format!("{}{}", prefix, error));
        }
        parts.join(DETAIL_SEPARATOR)
    };

    let base_detail = if state.stale {
        stale_detail
    } else {
        state.error.clone().unwrap_or_else(|| kind.clone())
    };

    let mut detail = match &state.reason {
        Some(reason) => format!("{}{}reason {}", base_detail, DETAIL_SEPARATOR, enum_str(reason)),
        None => base_detail,
    };
    if let Some(retry_after) = &state.retry_after {
        detail = format!("{} retry after {}", detail, retry_after);
    }

    Some(attention_row(
        provider,
        "all",
        &kind,
        detail,
        state.remedy_command.as_deref().unwrap_or(NONE),
    ))
}

/// State both sides of a bound conflict, so the reason the scope has no number
/// is the contradiction itself rather than a bare list of blocking windows.
fn bound_conflict_detail(conflict: &BoundConflict) -> String {
    let exhausted = join_ids(&conflict.exhausted_window_ids).unwrap_or_else(|| UNKNOWN.to_string());
    let live = join_ids(&conflict.live_window_ids).unwrap_or_else(|| UNKNOWN.to_string());
    format!(
        "{} reads 0{}{} still report allowance",
        exhausted, DETAIL_SEPARATOR, live
    )
}

/// Which windows suppress the scope's headroom, so absence is explained.
fn headroom_blockers(scope: &EffectiveAvailability) -> String {
    join_ids(&unmeasurable_ids(scope))
        .or_else(|| join_ids(&scope.bounded_by))
        .unwrap_or_else(|| UNKNOWN.to_string())
}

fn unknown_headroom_detail(scope: &EffectiveAvailability) -> String {
    let blockers = headroom_blockers(scope);
    if let Some(runway) = &scope.runway {
        if runway.status == RunwayStatus::ExhaustedNow
            || runway.status == RunwayStatus::ProjectedExhaustion
        {
            let limiting = runway.limiting_window_id.as_deref().unwrap_or(UNKNOWN);
            return format!(
                "{}{}{} limited by {}",
                blockers,
                DETAIL_SEPARATOR,
                enum_str(&runway.status),
                limiting
            );
        }
    }
    blockers
}

/// Which windows block a derived signal on a scope that does report headroom.
fn blocked_signals(scope: &EffectiveAvailability) -> Option<String> {
    let runway_ids = scope
        .runway
        .as_ref()
        .and_then(|r| r.unmeasurable_window_ids.as_deref())
        .unwrap_or(&[]);
    let selection_ids = scope
        .selection
        .as_ref()
        .and_then(|s| s.unmeasurable_window_ids.as_deref())
        .unwrap_or(&[]);
    if runway_ids.is_empty() && selection_ids.is_empty() {
        return None;
    }
    if !runway_ids.is_empty() && same_ids(runway_ids, selection_ids) {
        return Some(format!(
            "{} blocks runway + {}",
            join_ids(runway_ids).unwrap(),
            SELECTION_SCALAR_KEY
        ));
    }
    let mut segments: Vec<String> = Vec::new();
    if !runway_ids.is_empty() {
        segments.push(format!("{} blocks runway", join_ids(runway_ids).unwrap()));
    }
    if !selection_ids.is_empty() {
        segments.push(format!(
            "{} blocks {}",
            join_ids(selection_ids).unwrap(),
            SELECTION_SCALAR_KEY
        ));
    }
    Some(segments.join(DETAIL_SEPARATOR))
}

fn unmeasurable_ids(scope: &EffectiveAvailability) -> Vec<String> {
    let mut seen: Vec<String> = Vec::new();
    let runway_ids = scope
        .runway
        .as_ref()
        .and_then(|r| r.unmeasurable_window_ids.as_deref())
        .unwrap_or(&[]);
    let selection_ids = scope
        .selection
        .as_ref()
        .and_then(|s| s.unmeasurable_window_ids.as_deref())
        .unwrap_or(&[]);
    for id in runway_ids.iter().chain(selection_ids.iter()) {
        if !seen.contains(id) {
            seen.push(id.clone());
        }
    }
    seen
}

fn same_ids(left: &[String], right: &[String]) -> bool {
    left.len() == right.len() && left.iter().zip(right).all(|(a, b)| a == b)
}

/// The binding window's own reset: the one fact that made `windows[]` load-bearing.
fn binding_reset(windows: &[QuotaWindow], scope: &EffectiveAvailability) -> String {
    for id in scope.limiting_window_ids.iter().flatten() {
        if let Some(window) = windows.iter().find(|candidate| candidate.id == *id) {
            if let Some(reset) = window.resets_at.as_deref().or(window.reset_text.as_deref()) {
                return reset.to_string();
            }
        }
    }
    UNKNOWN.to_string()
}

fn join_ids(ids: &[String]) -> Option<String> {
    if ids.is_empty() {
        None
    } else {
        Some(ids.join(ID_SEPARATOR))
    }
}

/// `--full` audit tier: every derivation input the lean blocks summarize.
fn audit_blocks(response: &QuotaAxiResponse) -> Vec<String> {
    vec![
        encode_tabular(&Tabular {
            key: "providers",
            fields: PROVIDERS_FIELDS,
            rows: providers_rows(response),
        }),
        encode_tabular(&Tabular {
            key: "windows",
            fields: WINDOWS_FIELDS,
            rows: windows_rows(response),
        }),
        encode_tabular(&Tabular {
            key: "scopeAudit",
            fields: SCOPE_AUDIT_FIELDS,
            rows: scope_audit_rows(response),
        }),
        encode_tabular(&Tabular {
            key: "accounts",
            fields: ACCOUNTS_FIELDS,
            rows: accounts_rows(response),
        }),
        encode_tabular(&Tabular {
            key: "attempts",
            fields: ATTEMPTS_FIELDS,
            rows: attempts_rows(response),
        }),
    ]
}

fn providers_rows(response: &QuotaAxiResponse) -> Vec<Vec<Prim>> {
    response
        .providers
        .iter()
        .map(|provider| {
            vec![
                Prim::Str(provider.provider.as_str().to_string()),
                Prim::Str(provider.plan.clone().unwrap_or_else(|| UNKNOWN.to_string())),
                Prim::Str(
                    provider
                        .source
                        .as_ref()
                        .map(enum_str)
                        .unwrap_or_else(|| UNKNOWN.to_string()),
                ),
                Prim::Str(enum_str(&provider.state.status)),
                Prim::Str(
                    provider
                        .state
                        .auth_status
                        .as_ref()
                        .map(enum_str)
                        .unwrap_or_else(|| UNKNOWN.to_string()),
                ),
                Prim::Str(
                    provider
                        .quota_semantics
                        .as_ref()
                        .map(|s| enum_str(&s.status))
                        .unwrap_or_else(|| UNKNOWN.to_string()),
                ),
                Prim::Str(
                    provider
                        .state
                        .refreshed_at
                        .clone()
                        .unwrap_or_else(|| NONE.to_string()),
                ),
            ]
        })
        .collect()
}

fn windows_rows(response: &QuotaAxiResponse) -> Vec<Vec<Prim>> {
    let mut rows = Vec::new();
    for provider in &response.providers {
        for window in &provider.windows {
            let pace = window.pace.as_ref();
            rows.push(vec![
                Prim::Str(provider.provider.as_str().to_string()),
                Prim::Str(window.id.clone()),
                Prim::Str(window.label.clone()),
                opt_num(window.percent_remaining),
                Prim::Str(
                    window
                        .resets_at
                        .clone()
                        .or_else(|| window.reset_text.clone())
                        .unwrap_or_else(|| UNKNOWN.to_string()),
                ),
                Prim::Str(
                    pace.map(|p| enum_str(&p.status))
                        .unwrap_or_else(|| UNKNOWN.to_string()),
                ),
                opt_num(pace.and_then(|p| p.reserve_percent_points)),
                opt_num(pace.and_then(|p| p.burn_multiple)),
                opt_num(pace.and_then(|p| p.time_remaining_percent)),
                opt_num(pace.and_then(|p| p.elapsed_percent)),
                opt_num(pace.and_then(|p| p.cycle_seconds)),
                Prim::Str(
                    pace.and_then(|p| p.projected_exhausted_at.clone())
                        .unwrap_or_else(|| UNKNOWN.to_string()),
                ),
                Prim::Str(
                    pace.and_then(|p| p.projection_confidence.as_ref())
                        .map(enum_str)
                        .unwrap_or_else(|| UNKNOWN.to_string()),
                ),
            ]);
        }
    }
    rows
}

fn scope_audit_rows(response: &QuotaAxiResponse) -> Vec<Vec<Prim>> {
    let mut rows = Vec::new();
    for provider in &response.providers {
        let scopes = provider
            .quota_semantics
            .as_ref()
            .map(|s| s.effective_availability.as_slice())
            .unwrap_or(&[]);
        for scope in scopes {
            let pace = scope.pace.as_ref();
            rows.push(vec![
                Prim::Str(provider.provider.as_str().to_string()),
                Prim::Str(scope.scope.clone()),
                Prim::Str(if scope.bounded_by.is_empty() {
                    NONE.to_string()
                } else {
                    scope.bounded_by.join(ID_SEPARATOR)
                }),
                Prim::Str(
                    provider
                        .quota_semantics
                        .as_ref()
                        .map(|s| enum_str(&s.status))
                        .unwrap_or_else(|| UNKNOWN.to_string()),
                ),
                Prim::Str(
                    pace.map(|p| enum_str(&p.status))
                        .unwrap_or_else(|| UNKNOWN.to_string()),
                ),
                Prim::Str(
                    join_ids(pace.and_then(|p| p.ahead_window_ids.as_deref()).unwrap_or(&[]))
                        .unwrap_or_else(|| NONE.to_string()),
                ),
                Prim::Str(
                    join_ids(pace.and_then(|p| p.behind_window_ids.as_deref()).unwrap_or(&[]))
                        .unwrap_or_else(|| NONE.to_string()),
                ),
                Prim::Str(
                    join_ids(pace.and_then(|p| p.on_pace_window_ids.as_deref()).unwrap_or(&[]))
                        .unwrap_or_else(|| NONE.to_string()),
                ),
                Prim::Str(
                    join_ids(pace.and_then(|p| p.unknown_window_ids.as_deref()).unwrap_or(&[]))
                        .unwrap_or_else(|| NONE.to_string()),
                ),
                opt_num(pace.and_then(|p| p.worst_reserve_percent_points)),
                Prim::Str(
                    pace.and_then(|p| p.worst_reserve_window_id.clone())
                        .unwrap_or_else(|| UNKNOWN.to_string()),
                ),
            ]);
        }
    }
    rows
}

fn accounts_rows(response: &QuotaAxiResponse) -> Vec<Vec<Prim>> {
    response
        .providers
        .iter()
        .map(|provider| {
            let account = provider.account.as_ref();
            vec![
                Prim::Str(provider.provider.as_str().to_string()),
                Prim::Str(
                    account
                        .and_then(|a| a.email.clone())
                        .unwrap_or_else(|| "hidden".to_string()),
                ),
                opt_string(
                    account.and_then(|a| a.organization.clone()),
                    NONE,
                ),
                opt_string(account.and_then(|a| a.account_id.clone()), NONE),
                Prim::Str(
                    account
                        .and_then(|a| a.identity_status.as_ref())
                        .map(enum_str)
                        .unwrap_or_else(|| UNKNOWN.to_string()),
                ),
            ]
        })
        .collect()
}

fn attempts_rows(response: &QuotaAxiResponse) -> Vec<Vec<Prim>> {
    let mut rows = Vec::new();
    for provider in &response.providers {
        for attempt in provider.attempts.as_deref().unwrap_or(&[]) {
            rows.push(vec![
                Prim::Str(provider.provider.as_str().to_string()),
                Prim::Str(attempt.source.clone()),
                Prim::Str(enum_str(&attempt.status)),
                opt_string(attempt.error.clone(), "none"),
            ]);
        }
    }
    rows
}

/// `collapseHome` (Unix): render `$HOME` as `~`, and home-relative paths as
/// `~/…`. Non-absolute, non-home paths pass through unchanged.
fn collapse_home(path: &str) -> String {
    let home = std::env::var("HOME").unwrap_or_default();
    if home.is_empty() {
        return path.to_string();
    }
    if path == home {
        return "~".to_string();
    }
    if !is_absolute(path) && !starts_with_home_prefix(path, &home) {
        return path.to_string();
    }
    let relative = relative_path(&home, path);
    if relative.is_empty() {
        return "~".to_string();
    }
    if is_home_relative_path(&relative) {
        return format!("~/{}", relative);
    }
    if starts_with_home_prefix(path, &home) {
        return format!("~/{}", &path[home.len() + 1..]);
    }
    path.to_string()
}

fn is_absolute(path: &str) -> bool {
    std::path::Path::new(path).is_absolute()
}

fn starts_with_home_prefix(path: &str, home: &str) -> bool {
    if path.len() <= home.len() || !path.is_char_boundary(home.len()) {
        return false;
    }
    let separator = path.as_bytes()[home.len()] as char;
    (separator == '/' || separator == '\\') && &path[..home.len()] == home
}

fn is_home_relative_path(path: &str) -> bool {
    path != ".." && !path.starts_with("../") && !is_absolute(path)
}

fn relative_path(home: &str, path: &str) -> String {
    let home_path = std::path::Path::new(home);
    let path = std::path::Path::new(path);
    match path.strip_prefix(home_path) {
        Ok(relative) => relative.to_string_lossy().into_owned(),
        Err(_) => path.to_string_lossy().into_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::*;

    fn response_with(provider: ProviderQuota) -> QuotaAxiResponse {
        QuotaAxiResponse {
            generated_at: "2026-07-06T18:10:00Z".to_string(),
            schema_version: 5,
            providers: vec![provider],
            help: None,
        }
    }

    fn unavailable_agy() -> ProviderQuota {
        ProviderQuota {
            provider: ProviderId::Agy,
            label: Some("Antigravity".to_string()),
            source: Some(ProviderSource::Unavailable),
            plan: None,
            account: None,
            windows: vec![],
            quota_semantics: None,
            credits: None,
            state: ProviderState {
                status: ProviderStatus::Unavailable,
                stale: false,
                refreshed_at: None,
                error: Some("Antigravity/agy is not running".to_string()),
                retry_after: None,
                auth_status: None,
                reason: None,
                remedy_command: None,
                untrusted_window_ids: None,
                degraded_sources: None,
                sources_tried: Some(vec!["loopback".to_string()]),
            },
            attempts: None,
        }
    }

    /// A fresh agy reading: interpretation yields `unknownSemantics` for agy,
    /// so every window lands in `unresolvedWindowIds` and no scope is measurable.
    fn fresh_agy_unknown() -> ProviderQuota {
        ProviderQuota {
            provider: ProviderId::Agy,
            label: Some("Antigravity".to_string()),
            source: None,
            plan: None,
            account: None,
            windows: vec![QuotaWindow {
                id: "credits".to_string(),
                label: "credits".to_string(),
                kind: WindowKind::Credits,
                percent_used: None,
                percent_remaining: Some(100.0),
                starts_at: None,
                resets_at: None,
                reset_text: None,
                window_seconds: None,
                spent_usd: None,
                limit_usd: None,
                pace: None,
            }],
            quota_semantics: Some(QuotaSemantics {
                status: SemanticsStatus::Unknown,
                description: None,
                effective_availability: vec![],
                unresolved_window_ids: Some(vec!["credits".to_string()]),
            }),
            credits: None,
            state: ProviderState {
                status: ProviderStatus::Fresh,
                stale: false,
                refreshed_at: None,
                error: None,
                retry_after: None,
                auth_status: None,
                reason: None,
                remedy_command: None,
                untrusted_window_ids: None,
                degraded_sources: None,
                sources_tried: None,
            },
            attempts: None,
        }
    }

    /// A measurable Kimi-shaped scope, matching the `freshKimiQuota` reference:
    /// `kimi,all_models,67.5,unknown,unknown,unknown,weekly,"…reset…"`.
    fn kimi_measurable() -> ProviderQuota {
        ProviderQuota {
            provider: ProviderId::Kimi,
            label: Some("Kimi".to_string()),
            source: Some(ProviderSource::Api),
            plan: None,
            account: None,
            windows: vec![QuotaWindow {
                id: "weekly".to_string(),
                label: "week".to_string(),
                kind: WindowKind::Weekly,
                percent_used: Some(32.5),
                percent_remaining: Some(67.5),
                starts_at: None,
                resets_at: Some("2027-02-08T04:05:06.000Z".to_string()),
                reset_text: None,
                window_seconds: None,
                spent_usd: None,
                limit_usd: None,
                pace: None,
            }],
            quota_semantics: Some(QuotaSemantics {
                status: SemanticsStatus::Known,
                description: None,
                effective_availability: vec![EffectiveAvailability {
                    scope: "all_models".to_string(),
                    status: AvailabilityStatus::Known,
                    effective_percent_remaining: Some(67.5),
                    bounded_by: vec!["weekly".to_string()],
                    limiting_window_ids: Some(vec!["weekly".to_string()]),
                    bound_conflict: None,
                    pace: None,
                    runway: Some(EffectiveRunway {
                        status: RunwayStatus::Unknown,
                        usable_runway_seconds: None,
                        projected_exhausted_at: None,
                        limiting_window_id: None,
                        projection_confidence: None,
                        unmeasurable_window_ids: None,
                    }),
                    selection: Some(EffectiveSelection {
                        status: SelectionStatus::Unknown,
                        spend_priority: None,
                        unmeasurable_window_ids: None,
                    }),
                }],
                unresolved_window_ids: None,
            }),
            credits: None,
            state: ProviderState {
                status: ProviderStatus::Fresh,
                stale: false,
                refreshed_at: None,
                error: None,
                retry_after: None,
                auth_status: None,
                reason: None,
                remedy_command: None,
                untrusted_window_ids: None,
                degraded_sources: None,
                sources_tried: None,
            },
            attempts: None,
        }
    }

    /// One measurable scope at a finite runway point plus a bound-conflict model
    /// scope, exercising `quota[]`, `exhaustion[]`, and `attention[]` together.
    fn codex_bound_conflict_and_exhaustion() -> ProviderQuota {
        ProviderQuota {
            provider: ProviderId::Codex,
            label: Some("Codex".to_string()),
            source: Some(ProviderSource::CliRpc),
            plan: Some("pro".to_string()),
            account: None,
            windows: vec![
                QuotaWindow {
                    id: "five_hour".to_string(),
                    label: "session".to_string(),
                    kind: WindowKind::Session,
                    percent_used: Some(8.0),
                    percent_remaining: Some(92.0),
                    starts_at: Some("2026-09-07T05:27:00.000Z".to_string()),
                    resets_at: Some("2026-09-07T10:27:00.000Z".to_string()),
                    reset_text: None,
                    window_seconds: None,
                    spent_usd: None,
                    limit_usd: None,
                    pace: None,
                },
                QuotaWindow {
                    id: "weekly".to_string(),
                    label: "week".to_string(),
                    kind: WindowKind::Weekly,
                    percent_used: Some(100.0),
                    percent_remaining: Some(0.0),
                    starts_at: Some("2026-08-31T17:27:00.000Z".to_string()),
                    resets_at: Some("2026-09-07T17:27:00.000Z".to_string()),
                    reset_text: None,
                    window_seconds: None,
                    spent_usd: None,
                    limit_usd: None,
                    pace: None,
                },
            ],
            quota_semantics: Some(QuotaSemantics {
                status: SemanticsStatus::Partial,
                description: None,
                effective_availability: vec![
                    EffectiveAvailability {
                        scope: "all_models".to_string(),
                        status: AvailabilityStatus::Known,
                        effective_percent_remaining: Some(47.0),
                        bounded_by: vec!["weekly".to_string()],
                        limiting_window_ids: Some(vec!["weekly".to_string()]),
                        bound_conflict: None,
                        pace: None,
                        runway: Some(EffectiveRunway {
                            status: RunwayStatus::ProjectedExhaustion,
                            usable_runway_seconds: Some(10365.0),
                            projected_exhausted_at: Some("2026-03-19T03:43:45.600Z".to_string()),
                            limiting_window_id: Some("weekly".to_string()),
                            projection_confidence: Some(ProjectionConfidence::Established),
                            unmeasurable_window_ids: None,
                        }),
                        selection: Some(EffectiveSelection {
                            status: SelectionStatus::Known,
                            spend_priority: Some(0.5),
                            unmeasurable_window_ids: None,
                        }),
                    },
                    EffectiveAvailability {
                        scope: "model:codex_bengalfox".to_string(),
                        status: AvailabilityStatus::Unknown,
                        effective_percent_remaining: None,
                        bounded_by: vec![
                            "weekly".to_string(),
                            "model:codex_bengalfox:5h".to_string(),
                            "model:codex_bengalfox:7d".to_string(),
                        ],
                        limiting_window_ids: None,
                        bound_conflict: Some(BoundConflict {
                            exhausted_window_ids: vec!["weekly".to_string()],
                            live_window_ids: vec![
                                "model:codex_bengalfox:5h".to_string(),
                                "model:codex_bengalfox:7d".to_string(),
                            ],
                        }),
                        pace: None,
                        runway: None,
                        selection: None,
                    },
                ],
                unresolved_window_ids: None,
            }),
            credits: None,
            state: ProviderState {
                status: ProviderStatus::Fresh,
                stale: false,
                refreshed_at: None,
                error: None,
                retry_after: None,
                auth_status: None,
                reason: None,
                remedy_command: None,
                untrusted_window_ids: None,
                degraded_sources: None,
                sources_tried: None,
            },
            attempts: None,
        }
    }

    #[test]
    fn failed_agy_toon_is_byte_for_byte() {
        let response = response_with(unavailable_agy());
        let output = render_quota_toon(&response, "quota-axi", false);
        assert_eq!(
            output,
            concat!(
                "bin: quota-axi\n",
                "description: Report local agent-provider quota windows for routing-aware agents\n",
                "generatedAt: \"2026-07-06T18:10:00Z\"\n",
                "quota[0]:\n",
                "exhaustion[0]:\n",
                "attention[1]{provider,scope,kind,detail,remedy}:\n",
                "  agy,all,unavailable,Antigravity/agy is not running,none\n",
                "help[1]:\n",
                "  Run `quota-axi --full` for windows, pace, reserve, and account evidence",
            )
        );
    }

    #[test]
    fn failed_agy_full_toon_adds_audit_blocks() {
        let response = response_with(unavailable_agy());
        let output = render_quota_toon(&response, "quota-axi", true);
        assert_eq!(
            output,
            concat!(
                "bin: quota-axi\n",
                "description: Report local agent-provider quota windows for routing-aware agents\n",
                "generatedAt: \"2026-07-06T18:10:00Z\"\n",
                "quota[0]:\n",
                "exhaustion[0]:\n",
                "attention[1]{provider,scope,kind,detail,remedy}:\n",
                "  agy,all,unavailable,Antigravity/agy is not running,none\n",
                "providers[1]{provider,plan,source,status,authStatus,relationships,refreshedAt}:\n",
                "  agy,unknown,unavailable,unavailable,unknown,unknown,none\n",
                "windows[0]:\n",
                "scopeAudit[0]:\n",
                "accounts[1]{provider,email,organization,accountId,identityStatus}:\n",
                "  agy,hidden,none,none,unknown\n",
                "attempts[0]:\n",
                "help[1]:\n",
                "  Run `quota-axi --full` for windows, pace, reserve, and account evidence",
            )
        );
    }

    #[test]
    fn fresh_agy_unknown_semantics_is_unresolved_windows() {
        let response = QuotaAxiResponse {
            generated_at: "2026-09-07T05:27:00Z".to_string(),
            schema_version: 5,
            providers: vec![fresh_agy_unknown()],
            help: None,
        };
        let output = render_quota_toon(&response, "quota-axi", false);
        assert_eq!(
            output,
            concat!(
                "bin: quota-axi\n",
                "description: Report local agent-provider quota windows for routing-aware agents\n",
                "generatedAt: \"2026-09-07T05:27:00Z\"\n",
                "quota[0]:\n",
                "exhaustion[0]:\n",
                "attention[1]{provider,scope,kind,detail,remedy}:\n",
                "  agy,all,unresolved_windows,credits,none\n",
                "help[1]:\n",
                "  Run `quota-axi --full` for windows, pace, reserve, and account evidence",
            )
        );
    }

    #[test]
    fn measurable_scope_quota_row_is_byte_for_byte() {
        let response = QuotaAxiResponse {
            generated_at: "2027-02-03T04:05:06.000Z".to_string(),
            schema_version: 5,
            providers: vec![kimi_measurable()],
            help: None,
        };
        let output = render_quota_toon(&response, "quota-axi", false);
        assert_eq!(
            output,
            concat!(
                "bin: quota-axi\n",
                "description: Report local agent-provider quota windows for routing-aware agents\n",
                "generatedAt: \"2027-02-03T04:05:06.000Z\"\n",
                "quota[1]{provider,scope,effectivePercentRemaining,spendPriority,runway,confidence,limitedBy,resetsAt}:\n",
                "  kimi,all_models,67.5,unknown,unknown,unknown,weekly,\"2027-02-08T04:05:06.000Z\"\n",
                "exhaustion[0]:\n",
                "attention[0]:\n",
                "help[1]:\n",
                "  Run `quota-axi --full` for windows, pace, reserve, and account evidence",
            )
        );
    }

    #[test]
    fn bound_conflict_and_exhaustion_blocks_are_byte_for_byte() {
        let response = QuotaAxiResponse {
            generated_at: "2026-09-07T05:27:00.000Z".to_string(),
            schema_version: 5,
            providers: vec![codex_bound_conflict_and_exhaustion()],
            help: None,
        };
        let output = render_quota_toon(&response, "quota-axi", false);
        assert_eq!(
            output,
            concat!(
                "bin: quota-axi\n",
                "description: Report local agent-provider quota windows for routing-aware agents\n",
                "generatedAt: \"2026-09-07T05:27:00.000Z\"\n",
                "quota[1]{provider,scope,effectivePercentRemaining,spendPriority,runway,confidence,limitedBy,resetsAt}:\n",
                "  codex,all_models,47,0.5,projected_exhaustion,established,weekly,\"2026-09-07T17:27:00.000Z\"\n",
                "exhaustion[1]{provider,scope,usableRunwaySeconds,projectedExhaustedAt,limitingWindowId}:\n",
                "  codex,all_models,10365,\"2026-03-19T03:43:45.600Z\",weekly\n",
                "attention[1]{provider,scope,kind,detail,remedy}:\n",
                "  codex,\"model:codex_bengalfox\",bound_conflict,\"weekly reads 0 · model:codex_bengalfox:5h + model:codex_bengalfox:7d still report allowance\",none\n",
                "help[1]:\n",
                "  Run `quota-axi --full` for windows, pace, reserve, and account evidence",
            )
        );
    }

    #[test]
    fn redaction_hides_account_and_attempts_unless_full() {
        let provider = ProviderQuota {
            account: Some(Account {
                email: Some("person@example.invalid".to_string()),
                organization: None,
                account_id: None,
                identity_status: None,
            }),
            attempts: Some(vec![SourceAttempt {
                source: "oauth".to_string(),
                status: AttemptStatus::Success,
                error: None,
                credential_present: None,
                degraded: None,
            }]),
            ..unavailable_agy()
        };
        let response = response_with(provider);

        let redacted = redacted_response(&response, false);
        assert!(redacted.providers[0].account.is_none());
        assert!(redacted.providers[0].attempts.is_none());

        let full = redacted_response(&response, true);
        assert_eq!(
            full.providers[0].account.as_ref().unwrap().email.as_deref(),
            Some("person@example.invalid")
        );
        assert!(full.providers[0].attempts.is_some());
    }

    #[test]
    fn json_lean_matches_node_demotion_for_unavailable_agy() {
        let response = response_with(unavailable_agy());
        let lean = quota_json_report(&response, false);
        assert_eq!(
            lean,
            serde_json::json!({
                "generatedAt": "2026-07-06T18:10:00Z",
                "schemaVersion": 5,
                "providers": [{
                    "provider": "agy",
                    "windows": [],
                    "state": {
                        "status": "unavailable",
                        "stale": false,
                        "error": "Antigravity/agy is not running"
                    }
                }]
            })
        );
        let full = quota_json_report(&response, true);
        assert_eq!(
            full,
            serde_json::json!({
                "generatedAt": "2026-07-06T18:10:00Z",
                "schemaVersion": 5,
                "providers": [{
                    "provider": "agy",
                    "label": "Antigravity",
                    "source": "unavailable",
                    "windows": [],
                    "state": {
                        "status": "unavailable",
                        "stale": false,
                        "error": "Antigravity/agy is not running",
                        "sourcesTried": ["loopback"]
                    }
                }]
            })
        );
    }

    #[test]
    fn json_lean_demotes_derivation_inputs_only() {
        let provider = ProviderQuota {
            provider: ProviderId::Claude,
            label: Some("Claude".to_string()),
            source: Some(ProviderSource::Oauth),
            plan: Some("pro".to_string()),
            account: Some(Account {
                email: Some("person@example.invalid".to_string()),
                organization: Some("ACME".to_string()),
                account_id: Some("act_1".to_string()),
                identity_status: Some(IdentityStatus::Verified),
            }),
            windows: vec![QuotaWindow {
                id: "five_hour".to_string(),
                label: "session".to_string(),
                kind: WindowKind::Session,
                percent_used: Some(10.0),
                percent_remaining: Some(90.0),
                starts_at: Some("2026-07-15T07:00:00.000Z".to_string()),
                resets_at: Some("2026-07-15T17:00:00.000Z".to_string()),
                reset_text: None,
                window_seconds: Some(36000.0),
                spent_usd: None,
                limit_usd: None,
                pace: Some(QuotaPace {
                    status: QuotaPaceStatus::OnPace,
                    reason: Some(QuotaPaceReason::Stale),
                    time_remaining_percent: Some(50.0),
                    elapsed_percent: Some(50.0),
                    reserve_percent_points: Some(40.0),
                    burn_multiple: Some(1.0),
                    projected_exhausted_at: Some("2026-07-15T17:00:00.000Z".to_string()),
                    projection_confidence: Some(ProjectionConfidence::Established),
                    cycle_basis: Some(CycleBasis::StartsAtResetsAt),
                    cycle_seconds: Some(36000.0),
                }),
            }],
            quota_semantics: Some(QuotaSemantics {
                status: SemanticsStatus::Known,
                description: Some("Claude account windows bind every model.".to_string()),
                effective_availability: vec![EffectiveAvailability {
                    scope: "all_models".to_string(),
                    status: AvailabilityStatus::Known,
                    effective_percent_remaining: Some(90.0),
                    bounded_by: vec!["five_hour".to_string()],
                    limiting_window_ids: Some(vec!["five_hour".to_string()]),
                    bound_conflict: None,
                    pace: Some(EffectivePaceSummary {
                        status: PaceSummaryStatus::OnPace,
                        ahead_window_ids: Some(vec!["five_hour".to_string()]),
                        behind_window_ids: Some(vec!["extra".to_string()]),
                        on_pace_window_ids: Some(vec!["five_hour".to_string()]),
                        unknown_window_ids: None,
                        worst_reserve_percent_points: Some(40.0),
                        worst_reserve_window_id: Some("five_hour".to_string()),
                    }),
                    runway: Some(EffectiveRunway {
                        status: RunwayStatus::ThroughReset,
                        usable_runway_seconds: Some(18000.0),
                        projected_exhausted_at: None,
                        limiting_window_id: Some("five_hour".to_string()),
                        projection_confidence: Some(ProjectionConfidence::Established),
                        unmeasurable_window_ids: None,
                    }),
                    selection: Some(EffectiveSelection {
                        status: SelectionStatus::Known,
                        spend_priority: Some(40.0),
                        unmeasurable_window_ids: None,
                    }),
                }],
                unresolved_window_ids: None,
            }),
            credits: None,
            state: ProviderState {
                status: ProviderStatus::Fresh,
                stale: false,
                refreshed_at: Some("2026-07-15T12:00:00.000Z".to_string()),
                error: None,
                retry_after: None,
                auth_status: Some(ProviderAuthStatus::Usable),
                reason: None,
                remedy_command: None,
                untrusted_window_ids: None,
                degraded_sources: None,
                sources_tried: Some(vec!["oauth".to_string()]),
            },
            attempts: Some(vec![SourceAttempt {
                source: "oauth".to_string(),
                status: AttemptStatus::Success,
                error: None,
                credential_present: None,
                degraded: None,
            }]),
        };
        let response = response_with(provider);

        let lean = serde_json::to_string(&quota_json_report(&response, false)).unwrap();
        let full = serde_json::to_string(&quota_json_report(&response, true)).unwrap();

        // Redaction drops account identity and attempts unless --full.
        assert!(!lean.contains("person@example.invalid"));
        assert!(!lean.contains("attempts"));
        assert!(full.contains("person@example.invalid"));
        assert!(full.contains("attempts"));

        // Derivation inputs are demoted at render only.
        assert!(!lean.contains("percentUsed"));
        assert!(full.contains("percentUsed"));
        assert!(!lean.contains("startsAt"));
        assert!(!lean.contains("windowSeconds"));
        assert!(!lean.contains("timeRemainingPercent"));
        assert!(!lean.contains("elapsedPercent"));
        assert!(!lean.contains("cycleBasis"));
        assert!(!lean.contains("cycleSeconds"));
        assert!(!lean.contains("behindWindowIds"));
        assert!(!lean.contains("onPaceWindowIds"));
        assert!(!lean.contains("description"));
        assert!(!lean.contains("refreshedAt"));
        assert!(!lean.contains("sourcesTried"));
        assert!(!lean.contains("\"label\":\"Claude\""));
        assert!(!lean.contains("\"source\":"));
        // `windows[].pace.projectionConfidence` is demoted, but the scope's own
        // runway confidence is eligibility evidence and survives the lean tier.
        assert!(lean.contains("projectionConfidence"));

        // Eligibility and uncertainty fields survive the lean tier.
        assert!(lean.contains("reservePercentPoints"));
        assert!(lean.contains("burnMultiple"));
        assert!(lean.contains("percentRemaining"));
        assert!(lean.contains("resetsAt"));
        assert!(lean.contains("\"reason\":\"stale\""));
        assert!(lean.contains("aheadWindowIds"));
        assert!(lean.contains("worstReservePercentPoints"));
    }
}
