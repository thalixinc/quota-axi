//! Quota semantics layer mirroring `src/interpretation.ts`: derives each
//! provider's effective availability, runway, pace, and selection signal from
//! its normalized windows, and stamps every window with its cycle-average pace.

use crate::pace::{
    compute_effective_runway, compute_window_pace, summarize_effective_pace,
    summarize_effective_selection,
};
use crate::providers::degraded_sources;
use crate::types::{
    AvailabilityStatus, BoundConflict, DegradedSource, EffectiveAvailability,
    EffectivePaceSummary, EffectiveRunway, EffectiveSelection, PaceSummaryStatus, ProviderId,
    ProviderQuota, ProviderStatus, QuotaSemantics, QuotaWindow, RunwayStatus, SelectionStatus,
    SemanticsStatus, WindowKind,
};
use std::collections::HashSet;

pub fn with_quota_semantics(provider: &mut ProviderQuota, generated_at: &str) {
    let stale = provider.state.stale;
    for window in provider.windows.iter_mut() {
        window.pace = compute_window_pace(window, generated_at, stale);
    }
    let semantics = semantics_for(provider, generated_at);
    if let Some(degraded) = superseded_sources(provider) {
        provider.state.degraded_sources = Some(degraded);
    }
    provider.quota_semantics = Some(if stale {
        stale_semantics(&semantics)
    } else {
        semantics
    });
}

/// Name the sources a working sibling superseded. Only a fresh reading can
/// supersede anything: when nothing worked, `state.error` and the report status
/// already carry the auth problem, and repeating every source there would bury
/// it rather than make it visible.
fn superseded_sources(provider: &ProviderQuota) -> Option<Vec<DegradedSource>> {
    if provider.state.stale || provider.state.status != ProviderStatus::Fresh {
        return None;
    }
    let degraded = degraded_sources(provider.attempts.as_deref());
    if degraded.is_empty() {
        None
    } else {
        Some(degraded)
    }
}

fn stale_semantics(semantics: &QuotaSemantics) -> QuotaSemantics {
    let status = if semantics.status == SemanticsStatus::Partial {
        SemanticsStatus::Partial
    } else {
        SemanticsStatus::Unknown
    };
    let effective_availability = semantics
        .effective_availability
        .iter()
        .map(|availability| {
            let bounded_by = availability.bounded_by.clone();
            let unmeasurable = if bounded_by.is_empty() {
                None
            } else {
                Some(bounded_by.clone())
            };

            let pace = match &availability.pace {
                Some(pace) => {
                    let unknown_window_ids = match &pace.unknown_window_ids {
                        Some(ids) => Some(ids.clone()),
                        None if !bounded_by.is_empty() => Some(bounded_by.clone()),
                        None => None,
                    };
                    Some(EffectivePaceSummary {
                        status: PaceSummaryStatus::Unknown,
                        ahead_window_ids: None,
                        behind_window_ids: None,
                        on_pace_window_ids: None,
                        unknown_window_ids,
                        worst_reserve_percent_points: None,
                        worst_reserve_window_id: None,
                    })
                }
                None if !bounded_by.is_empty() => Some(EffectivePaceSummary {
                    status: PaceSummaryStatus::Unknown,
                    ahead_window_ids: None,
                    behind_window_ids: None,
                    on_pace_window_ids: None,
                    unknown_window_ids: Some(bounded_by.clone()),
                    worst_reserve_percent_points: None,
                    worst_reserve_window_id: None,
                }),
                None => None,
            };

            EffectiveAvailability {
                scope: availability.scope.clone(),
                status: AvailabilityStatus::Unknown,
                effective_percent_remaining: None,
                bounded_by,
                limiting_window_ids: None,
                bound_conflict: None,
                pace,
                runway: Some(EffectiveRunway {
                    status: RunwayStatus::Unknown,
                    usable_runway_seconds: None,
                    projected_exhausted_at: None,
                    limiting_window_id: None,
                    projection_confidence: None,
                    unmeasurable_window_ids: unmeasurable.clone(),
                }),
                selection: Some(EffectiveSelection {
                    status: SelectionStatus::Unknown,
                    spend_priority: None,
                    unmeasurable_window_ids: unmeasurable,
                }),
            }
        })
        .collect();

    QuotaSemantics {
        status,
        description: Some(
            "The raw quota windows are stale diagnostic data, so effective remaining is unknown until the provider refreshes successfully."
                .to_string(),
        ),
        effective_availability,
        unresolved_window_ids: semantics.unresolved_window_ids.clone(),
    }
}

fn semantics_for(provider: &ProviderQuota, generated_at: &str) -> QuotaSemantics {
    match provider.provider {
        ProviderId::Claude => claude_semantics(&provider.windows, generated_at),
        ProviderId::Codex => codex_semantics(&provider.windows, generated_at),
        ProviderId::Grok => grok_semantics(&provider.windows, generated_at),
        ProviderId::Kimi => kimi_semantics(
            &provider.windows,
            provider.state.untrusted_window_ids.as_deref().unwrap_or(&[]),
            generated_at,
        ),
        ProviderId::Zai => zai_semantics(
            &provider.windows,
            provider.state.untrusted_window_ids.as_deref().unwrap_or(&[]),
            generated_at,
        ),
        ProviderId::Cursor => cursor_semantics(&provider.windows, generated_at),
        ProviderId::Copilot | ProviderId::Agy => unknown_semantics(
            &provider.windows,
            &format!(
                "quota-axi does not know whether {}'s reported windows are independent or jointly bounding, so it does not claim an effective remaining percentage.",
                provider.label.as_deref().unwrap_or(provider.provider.as_str())
            ),
        ),
        ProviderId::Alibaba => alibaba_semantics(&provider.windows, generated_at),
        ProviderId::OpencodeGo => unknown_semantics(
            &provider.windows,
            "OpenCode Go reports rolling, weekly, and monthly windows, but quota-axi has no provider evidence that they jointly bound all models, so it does not claim an effective combined percentage.",
        ),
    }
}

fn alibaba_semantics(windows: &[QuotaWindow], generated_at: &str) -> QuotaSemantics {
    let account: Vec<QuotaWindow> = windows
        .iter()
        .filter(|w| w.id == "weekly")
        .cloned()
        .collect();
    let model_windows: Vec<QuotaWindow> = windows
        .iter()
        .filter(|w| w.id.starts_with("model:"))
        .cloned()
        .collect();
    let unresolved: Vec<QuotaWindow> = windows
        .iter()
        .filter(|w| w.id != "weekly" && !w.id.starts_with("model:"))
        .cloned()
        .collect();
    let unresolved_ids: Vec<String> = unresolved.iter().map(|w| w.id.clone()).collect();

    let mut effective_availability: Vec<EffectiveAvailability> = Vec::new();
    if !account.is_empty() {
        effective_availability.push(if !unresolved.is_empty() {
            unresolved_availability("all_models", &account, &unresolved_ids)
        } else {
            availability("all_models", &account, generated_at, None)
        });
    }

    let mut models: Vec<(String, Vec<QuotaWindow>)> = Vec::new();
    for window in model_windows {
        let scope = window.label.clone();
        match models.iter_mut().find(|(s, _)| *s == scope) {
            Some((_, scoped)) => scoped.push(window),
            None => models.push((scope, vec![window])),
        }
    }

    for (scope, scoped) in &models {
        let model_scope = scoped
            .first()
            .map(|w| w.id.clone())
            .unwrap_or_else(|| scope.clone());
        effective_availability.push(if !unresolved.is_empty() {
            unresolved_availability(&model_scope, scoped, &unresolved_ids)
        } else {
            availability(&model_scope, scoped, generated_at, None)
        });
    }

    if !unresolved.is_empty() {
        return QuotaSemantics {
            status: SemanticsStatus::Partial,
            description: Some(
                "Alibaba's account weekly window binds the account scope, while model-scoped limits bind only their named model. Unfamiliar windows are not assigned to either scope, so effective percentages remain unknown."
                    .to_string(),
            ),
            effective_availability,
            unresolved_window_ids: Some(unresolved_ids),
        };
    }
    known_semantics(
        effective_availability,
        "Alibaba's account weekly window is available at account scope; model-scoped limits bind only their named model and never become an account-wide bound.",
    )
}

fn claude_semantics(windows: &[QuotaWindow], generated_at: &str) -> QuotaSemantics {
    let account: Vec<QuotaWindow> = windows
        .iter()
        .filter(|w| w.id == "five_hour" || w.id == "seven_day")
        .cloned()
        .collect();
    let models: Vec<QuotaWindow> = windows
        .iter()
        .filter(|w| w.kind == WindowKind::Model)
        .cloned()
        .collect();
    let unresolved: Vec<QuotaWindow> = windows
        .iter()
        .filter(|w| {
            !(w.id == "five_hour" || w.id == "seven_day" || w.id == "extra_usage")
                && w.kind != WindowKind::Model
        })
        .cloned()
        .collect();

    if !unresolved.is_empty() {
        return partial_semantics(
            &unresolved,
            "Claude account windows bound every model and model windows add another bound, but unfamiliar windows prevent a definitive effective percentage.",
        );
    }

    let mut effective_availability: Vec<EffectiveAvailability> = Vec::new();
    if !account.is_empty() {
        effective_availability.push(availability("all_models", &account, generated_at, None));
    }
    for model in &models {
        let mut bound = account.clone();
        bound.push(model.clone());
        effective_availability.push(availability(&model.id, &bound, generated_at, None));
    }
    known_semantics(
        effective_availability,
        "Claude account windows bound every model. A model-specific window is an additional bound, so that model's effective remaining percentage is the minimum across the named windows.",
    )
}

fn codex_semantics(windows: &[QuotaWindow], generated_at: &str) -> QuotaSemantics {
    let account: Vec<QuotaWindow> = windows
        .iter()
        .filter(|w| is_codex_account_window(w))
        .cloned()
        .collect();
    let code_review: Vec<QuotaWindow> = windows
        .iter()
        .filter(|w| is_code_review_window(w))
        .cloned()
        .collect();
    let model_windows: Vec<QuotaWindow> = windows
        .iter()
        .filter(|w| w.kind == WindowKind::Model)
        .cloned()
        .collect();

    let mut models: Vec<(String, Vec<QuotaWindow>)> = Vec::new();
    for window in model_windows {
        let scope = codex_model_scope(&window.id);
        match models.iter_mut().find(|(s, _)| *s == scope) {
            Some((_, scoped)) => scoped.push(window),
            None => models.push((scope, vec![window])),
        }
    }

    let unresolved: Vec<QuotaWindow> = windows
        .iter()
        .filter(|w| {
            !is_codex_account_window(w) && !is_code_review_window(w) && w.kind != WindowKind::Model
        })
        .cloned()
        .collect();

    if !unresolved.is_empty() {
        return partial_semantics(
            &unresolved,
            "Codex base account windows are applied as a bound to every model scope, including scopes that have named model windows of their own, and a named model window is an additional, separately metered budget the vendor reports alongside the base limit. A base window at zero while that model's own windows all still report allowance is published as a bound conflict rather than as the model's exhaustion. Unfamiliar windows prevent a definitive effective percentage.",
        );
    }

    let mut effective_availability: Vec<EffectiveAvailability> = Vec::new();
    if !account.is_empty() {
        effective_availability.push(availability("all_models", &account, generated_at, None));
    }
    if !code_review.is_empty() {
        effective_availability.push(availability("code_review", &code_review, generated_at, None));
    }
    for (scope, scoped) in &models {
        let mut bound = account.clone();
        bound.extend(scoped.iter().cloned());
        effective_availability.push(availability(
            scope,
            &bound,
            generated_at,
            Some(scoped.as_slice()),
        ));
    }
    known_semantics(
        effective_availability,
        "Codex base account windows are applied as a bound to every model scope, including scopes that have named model windows of their own, so that model's effective remaining percentage is the minimum across the named windows. A named model window is an additional, separately metered budget the vendor reports alongside the base limit, so a base window at zero while that model's own windows all still report allowance is a contradiction between the two readings and is published as a bound conflict rather than as the model's exhaustion. Code-review windows describe a separate workload and are not included in model availability.",
    )
}

fn grok_semantics(windows: &[QuotaWindow], generated_at: &str) -> QuotaSemantics {
    let shared: Vec<QuotaWindow> = windows
        .iter()
        .filter(|w| w.id == "credits")
        .cloned()
        .collect();
    let products: Vec<QuotaWindow> = windows
        .iter()
        .filter(|w| w.id.starts_with("product:"))
        .cloned()
        .collect();
    let unresolved: Vec<QuotaWindow> = windows
        .iter()
        .filter(|w| w.id != "credits" && !w.id.starts_with("product:"))
        .cloned()
        .collect();

    if !unresolved.is_empty() {
        return partial_semantics(
            &unresolved,
            "Grok's shared credits window bounds every product and each product window adds a product-specific bound, but unfamiliar windows prevent a definitive effective percentage.",
        );
    }

    let mut effective_availability: Vec<EffectiveAvailability> = Vec::new();
    if !shared.is_empty() {
        effective_availability.push(availability("all_products", &shared, generated_at, None));
    }
    for product in &products {
        let mut bound = shared.clone();
        bound.push(product.clone());
        effective_availability.push(availability(&product.id, &bound, generated_at, None));
    }
    known_semantics(
        effective_availability,
        "Grok's shared credits window bounds every product. A product window is an additional bound, so that product's effective remaining percentage is the minimum across the named windows.",
    )
}

fn kimi_semantics(
    windows: &[QuotaWindow],
    untrusted_window_ids: &[String],
    generated_at: &str,
) -> QuotaSemantics {
    let unresolved: Vec<QuotaWindow> = windows
        .iter()
        .filter(|w| w.id != "weekly" && w.id != "five_hour")
        .cloned()
        .collect();

    let mut unresolved_window_ids: Vec<String> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    for id in unresolved.iter().map(|w| w.id.clone()) {
        if seen.insert(id.clone()) {
            unresolved_window_ids.push(id);
        }
    }
    for id in untrusted_window_ids {
        if seen.insert(id.clone()) {
            unresolved_window_ids.push(id.clone());
        }
    }

    if !unresolved_window_ids.is_empty() {
        let recognized: Vec<QuotaWindow> = windows
            .iter()
            .filter(|w| w.id == "weekly" || w.id == "five_hour")
            .cloned()
            .collect();
        let effective_availability = if !recognized.is_empty() {
            vec![unresolved_availability(
                "all_models",
                &recognized,
                &unresolved_window_ids,
            )]
        } else {
            Vec::new()
        };
        return QuotaSemantics {
            status: SemanticsStatus::Partial,
            description: Some(
                "Kimi's valid weekly and five-hour account windows are known bounds, but unrecognized or unparsed limits may add bounds, so effective remaining is unknown."
                    .to_string(),
            ),
            effective_availability,
            unresolved_window_ids: Some(unresolved_window_ids),
        };
    }

    let effective_availability = if windows.is_empty() {
        Vec::new()
    } else {
        vec![availability("all_models", windows, generated_at, None)]
    };
    known_semantics(
        effective_availability,
        "Kimi's weekly and five-hour account windows jointly bound every model, so effective remaining is the minimum across the named windows.",
    )
}

/// Cursor IDE recognized windows all draw on the same plan billing cycle, so
/// quota-axi treats them as jointly bounding rather than independent. That is
/// the conservative reading: the effective remaining is the minimum across them,
/// which never overstates headroom even if a window later turns out to be
/// independent. Grok Bot weekly usage is a separate Cursor-account resource.
const CURSOR_IDE_WINDOW_IDS: [&str; 4] = ["included_usage", "auto_usage", "api_usage", "spend_limit"];
const CURSOR_GROK_BOT_WINDOW_ID: &str = "grok_bot";

fn cursor_semantics(windows: &[QuotaWindow], generated_at: &str) -> QuotaSemantics {
    let ide: Vec<QuotaWindow> = windows
        .iter()
        .filter(|w| CURSOR_IDE_WINDOW_IDS.contains(&w.id.as_str()))
        .cloned()
        .collect();
    let grok_bot: Vec<QuotaWindow> = windows
        .iter()
        .filter(|w| w.id == CURSOR_GROK_BOT_WINDOW_ID)
        .cloned()
        .collect();
    let unresolved: Vec<QuotaWindow> = windows
        .iter()
        .filter(|w| {
            !CURSOR_IDE_WINDOW_IDS.contains(&w.id.as_str())
                && w.id != CURSOR_GROK_BOT_WINDOW_ID
        })
        .cloned()
        .collect();

    let mut effective_availability: Vec<EffectiveAvailability> = Vec::new();
    if !ide.is_empty() {
        effective_availability.push(availability("all_models", &ide, generated_at, None));
    }
    if !grok_bot.is_empty() {
        effective_availability.push(availability("grok_bot", &grok_bot, generated_at, None));
    }
    if !unresolved.is_empty() {
        return QuotaSemantics {
            status: SemanticsStatus::Partial,
            description: Some(
                "Cursor's included, auto, API usage, and spend-limit windows jointly bound every model, so effective remaining is the minimum across those named windows. The Grok Bot weekly window is an independent resource. Unfamiliar windows are not folded into either bound, so they stay unresolved."
                    .to_string(),
            ),
            effective_availability,
            unresolved_window_ids: Some(unresolved.iter().map(|w| w.id.clone()).collect()),
        };
    }
    known_semantics(
        effective_availability,
        "Cursor's included, auto, API usage, and spend-limit windows jointly bound every model, so effective remaining is the minimum across those named windows. The Grok Bot weekly window is an independent resource.",
    )
}

fn zai_semantics(
    windows: &[QuotaWindow],
    untrusted_window_ids: &[String],
    generated_at: &str,
) -> QuotaSemantics {
    let token: Vec<QuotaWindow> = windows
        .iter()
        .filter(|w| w.id == "five_hour" || w.id == "weekly")
        .cloned()
        .collect();
    let tool: Vec<QuotaWindow> = windows
        .iter()
        .filter(|w| w.id == "mcp_month")
        .cloned()
        .collect();
    let unresolved: Vec<QuotaWindow> = windows
        .iter()
        .filter(|w| !(w.id == "five_hour" || w.id == "weekly" || w.id == "mcp_month"))
        .cloned()
        .collect();

    let mut unresolved_window_ids: Vec<String> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    for id in unresolved.iter().map(|w| w.id.clone()) {
        if seen.insert(id.clone()) {
            unresolved_window_ids.push(id);
        }
    }
    for id in untrusted_window_ids {
        if seen.insert(id.clone()) {
            unresolved_window_ids.push(id.clone());
        }
    }

    if !unresolved_window_ids.is_empty() {
        let mut effective_availability: Vec<EffectiveAvailability> = Vec::new();
        if !token.is_empty() {
            effective_availability.push(unresolved_availability(
                "all_models",
                &token,
                &unresolved_window_ids,
            ));
        }
        if !tool.is_empty() {
            effective_availability.push(unresolved_availability(
                "tools",
                &tool,
                &unresolved_window_ids,
            ));
        }
        return QuotaSemantics {
            status: SemanticsStatus::Partial,
            description: Some(
                "Z.AI's five-hour and weekly token windows jointly bound model usage and the monthly tool window is a separate resource, but unfamiliar windows prevent a definitive effective percentage."
                    .to_string(),
            ),
            effective_availability,
            unresolved_window_ids: Some(unresolved_window_ids),
        };
    }

    let mut effective_availability: Vec<EffectiveAvailability> = Vec::new();
    if !token.is_empty() {
        effective_availability.push(availability("all_models", &token, generated_at, None));
    }
    if !tool.is_empty() {
        effective_availability.push(availability("tools", &tool, generated_at, None));
    }
    known_semantics(
        effective_availability,
        "Z.AI's five-hour and weekly token windows jointly bound model usage, so effective remaining is the minimum across the named windows. The monthly tool window is an independent resource.",
    )
}

/// Report a scope whose recognized windows are real bounds while unfamiliar
/// windows may add further bounds, so the effective percentage stays unknown and
/// every window that could bind the scope is named as unmeasurable.
fn unresolved_availability(
    scope: &str,
    windows: &[QuotaWindow],
    unresolved_window_ids: &[String],
) -> EffectiveAvailability {
    let bounded_by: Vec<String> = windows.iter().map(|w| w.id.clone()).collect();
    let mut unmeasurable = bounded_by.clone();
    unmeasurable.extend(unresolved_window_ids.iter().cloned());
    EffectiveAvailability {
        scope: scope.to_string(),
        status: AvailabilityStatus::Unknown,
        effective_percent_remaining: None,
        bounded_by,
        limiting_window_ids: None,
        bound_conflict: None,
        pace: Some(summarize_effective_pace(windows)),
        runway: Some(EffectiveRunway {
            status: RunwayStatus::Unknown,
            usable_runway_seconds: None,
            projected_exhausted_at: None,
            limiting_window_id: None,
            projection_confidence: None,
            unmeasurable_window_ids: Some(unmeasurable.clone()),
        }),
        selection: Some(EffectiveSelection {
            status: SelectionStatus::Unknown,
            spend_priority: None,
            unmeasurable_window_ids: Some(unmeasurable),
        }),
    }
}

/// A scope's own meter contradicting a bound it only inherits: the inherited
/// window reports zero remaining while every window metered for this scope alone
/// still reports allowance. Publishing the inherited zero as this scope's
/// effective remaining would assert an exhaustion the readings dispute, so the
/// caller reports the conflict instead of a settled number.
///
/// A zero on one of the scope's *own* windows is not a conflict: that is the
/// scope's own meter reporting exhaustion, which stands.
fn bound_conflict(windows: &[QuotaWindow], own_windows: &[QuotaWindow]) -> Option<BoundConflict> {
    if own_windows.is_empty() {
        return None;
    }
    let live = own_windows
        .iter()
        .all(|w| matches!(w.percent_remaining, Some(r) if r > 0.0));
    if !live {
        return None;
    }
    let own_ids: HashSet<&str> = own_windows.iter().map(|w| w.id.as_str()).collect();
    let exhausted: Vec<&QuotaWindow> = windows
        .iter()
        .filter(|w| !own_ids.contains(w.id.as_str()) && w.percent_remaining == Some(0.0))
        .collect();
    if exhausted.is_empty() {
        return None;
    }
    Some(BoundConflict {
        exhausted_window_ids: exhausted.iter().map(|w| w.id.clone()).collect(),
        live_window_ids: own_windows.iter().map(|w| w.id.clone()).collect(),
    })
}

fn availability(
    scope: &str,
    windows: &[QuotaWindow],
    generated_at: &str,
    // The subset of `windows` metered for this scope alone; the rest are bounds
    // inherited from a broader scope. Pass it only for a provider where the
    // inherited bound's enforcement over this scope is not established, so an
    // inherited zero that the scope's own meter contradicts is reported as a
    // conflict instead of as exhaustion.
    own_windows: Option<&[QuotaWindow]>,
) -> EffectiveAvailability {
    let bounded_by: Vec<String> = windows.iter().map(|w| w.id.clone()).collect();
    let pace = summarize_effective_pace(windows);
    let selection = summarize_effective_selection(windows);
    let conflict = own_windows.and_then(|own| bound_conflict(windows, own));

    if let Some(conflict) = conflict {
        // Both sides of the contradiction block the aggregate: the inherited zero
        // is not established over this scope, and the live own windows cannot
        // stand alone as the bound set either.
        let mut unmeasurable = conflict.exhausted_window_ids.clone();
        unmeasurable.extend(conflict.live_window_ids.iter().cloned());
        return EffectiveAvailability {
            scope: scope.to_string(),
            status: AvailabilityStatus::Unknown,
            effective_percent_remaining: None,
            bounded_by,
            limiting_window_ids: None,
            bound_conflict: Some(conflict),
            // Per-window pace is each window's own draw-down and stays true; it is
            // the very evidence that the own meter is live.
            pace: Some(pace),
            runway: Some(EffectiveRunway {
                status: RunwayStatus::Unknown,
                usable_runway_seconds: None,
                projected_exhausted_at: None,
                limiting_window_id: None,
                projection_confidence: None,
                unmeasurable_window_ids: Some(unmeasurable.clone()),
            }),
            selection: Some(EffectiveSelection {
                status: SelectionStatus::Unknown,
                spend_priority: None,
                unmeasurable_window_ids: Some(unmeasurable),
            }),
        };
    }

    if windows.is_empty() || windows.iter().any(|w| w.percent_remaining.is_none()) {
        return EffectiveAvailability {
            scope: scope.to_string(),
            status: AvailabilityStatus::Unknown,
            effective_percent_remaining: None,
            bounded_by,
            limiting_window_ids: None,
            bound_conflict: None,
            pace: Some(pace),
            runway: Some(compute_effective_runway(windows, generated_at)),
            selection: Some(selection),
        };
    }

    let effective_percent_remaining = windows
        .iter()
        .map(|w| w.percent_remaining.unwrap())
        .fold(f64::INFINITY, f64::min);
    let limiting_window_ids: Vec<String> = windows
        .iter()
        .filter(|w| w.percent_remaining == Some(effective_percent_remaining))
        .map(|w| w.id.clone())
        .collect();

    EffectiveAvailability {
        scope: scope.to_string(),
        status: AvailabilityStatus::Known,
        effective_percent_remaining: Some(effective_percent_remaining),
        bounded_by,
        limiting_window_ids: Some(limiting_window_ids),
        bound_conflict: None,
        pace: Some(pace),
        runway: Some(compute_effective_runway(windows, generated_at)),
        selection: Some(selection),
    }
}

fn is_codex_account_window(window: &QuotaWindow) -> bool {
    let id = window.id.as_str();
    if id.starts_with("window:") {
        return true;
    }
    for base in ["five_hour", "weekly"] {
        if let Some(suffix) = id.strip_prefix(base) {
            if suffix.is_empty() {
                return true;
            }
            if let Some(digits) = suffix.strip_prefix('_') {
                if !digits.is_empty() && digits.chars().all(|c| c.is_ascii_digit()) {
                    return true;
                }
            }
        }
    }
    false
}

fn is_code_review_window(window: &QuotaWindow) -> bool {
    window.id.starts_with("code_review_five_hour")
        || window.id.starts_with("code_review_weekly")
        || window.id.starts_with("code_review_window:")
}

fn codex_model_scope(id: &str) -> String {
    let mut scope = strip_trailing_digits(id);
    scope = strip_trailing_scope_suffix(&scope);
    scope
}

fn strip_trailing_digits(id: &str) -> String {
    if let Some(idx) = id.rfind('_') {
        let digits = &id[idx + 1..];
        if !digits.is_empty() && digits.chars().all(|c| c.is_ascii_digit()) {
            return id[..idx].to_string();
        }
    }
    id.to_string()
}

fn strip_trailing_scope_suffix(id: &str) -> String {
    for suffix in ["5h", "7d"] {
        if let Some(rest) = id.strip_suffix(&format!(":{suffix}")) {
            return rest.to_string();
        }
    }
    if let Some(idx) = id.rfind(":window:") {
        let prefix = &id[..idx];
        let rest = &id[idx + ":window:".len()..];
        if !rest.is_empty() && !rest.contains(':') {
            return prefix.to_string();
        }
    }
    id.to_string()
}

fn known_semantics(
    effective_availability: Vec<EffectiveAvailability>,
    description: &str,
) -> QuotaSemantics {
    if effective_availability.is_empty() {
        QuotaSemantics {
            status: SemanticsStatus::Unknown,
            description: Some(
                "No quota windows are available, so no effective remaining percentage can be computed."
                    .to_string(),
            ),
            effective_availability,
            unresolved_window_ids: None,
        }
    } else {
        QuotaSemantics {
            status: SemanticsStatus::Known,
            description: Some(description.to_string()),
            effective_availability,
            unresolved_window_ids: None,
        }
    }
}

fn partial_semantics(unresolved: &[QuotaWindow], description: &str) -> QuotaSemantics {
    QuotaSemantics {
        status: SemanticsStatus::Partial,
        description: Some(description.to_string()),
        effective_availability: Vec::new(),
        unresolved_window_ids: Some(unresolved.iter().map(|w| w.id.clone()).collect()),
    }
}

fn unknown_semantics(windows: &[QuotaWindow], description: &str) -> QuotaSemantics {
    QuotaSemantics {
        status: SemanticsStatus::Unknown,
        description: Some(if windows.is_empty() {
            "No quota windows are available, so no effective remaining percentage can be computed."
                .to_string()
        } else {
            description.to_string()
        }),
        effective_availability: Vec::new(),
        unresolved_window_ids: Some(windows.iter().map(|w| w.id.clone()).collect()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{
        AvailabilityStatus, ProviderId, ProviderQuota, ProviderSource, ProviderState, QuotaPace,
        QuotaPaceStatus, QuotaWindow, RunwayStatus, SelectionStatus, SemanticsStatus, WindowKind,
    };
    use chrono::{DateTime, SecondsFormat, Utc};

    const GENERATED_AT: &str = "2026-07-15T12:00:00.000Z";
    const WEEK_SECONDS: f64 = 604_800.0;

    fn generated_at_ms() -> f64 {
        DateTime::parse_from_rfc3339(GENERATED_AT)
            .unwrap()
            .timestamp_millis() as f64
    }

    fn iso(ms: f64) -> String {
        DateTime::<Utc>::from_timestamp_millis(ms as i64)
            .unwrap()
            .to_rfc3339_opts(SecondsFormat::Millis, true)
    }

    fn weekly_resets_at(elapsed_fraction: f64) -> String {
        iso(generated_at_ms() + WEEK_SECONDS * (1.0 - elapsed_fraction) * 1000.0)
    }

    fn provider(id: ProviderId, windows: Vec<QuotaWindow>) -> ProviderQuota {
        ProviderQuota {
            provider: id,
            label: Some(id.as_str().to_string()),
            source: Some(ProviderSource::Api),
            plan: None,
            account: None,
            windows,
            quota_semantics: None,
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
                sources_tried: Some(vec!["api".to_string()]),
            },
            attempts: None,
        }
    }

    fn window(id: &str, kind: WindowKind, percent_remaining: f64) -> QuotaWindow {
        QuotaWindow {
            id: id.to_string(),
            label: id.to_string(),
            kind,
            percent_used: Some(100.0 - percent_remaining),
            percent_remaining: Some(percent_remaining),
            starts_at: None,
            resets_at: None,
            reset_text: None,
            window_seconds: None,
            spent_usd: None,
            limit_usd: None,
            pace: None,
        }
    }

    #[test]
    fn copilot_and_agy_report_unknown_semantics() {
        for (id, window_id) in [
            (ProviderId::Copilot, "premium_interactions"),
            (ProviderId::Agy, "gemini_weekly"),
        ] {
            let mut p = provider(id, vec![window(window_id, WindowKind::Monthly, 100.0)]);
            with_quota_semantics(&mut p, GENERATED_AT);
            let semantics = p.quota_semantics.unwrap();
            assert_eq!(semantics.status, SemanticsStatus::Unknown);
            assert!(semantics.effective_availability.is_empty());
            assert_eq!(
                semantics.unresolved_window_ids,
                Some(vec![window_id.to_string()])
            );
        }
    }

    #[test]
    fn stale_provider_yields_unknown_semantics() {
        let mut p = provider(
            ProviderId::Claude,
            vec![window("five_hour", WindowKind::Session, 66.0)],
        );
        p.state.status = ProviderStatus::Stale;
        p.state.stale = true;
        p.state.refreshed_at = Some("2026-07-06T18:10:00Z".to_string());

        with_quota_semantics(&mut p, GENERATED_AT);
        let semantics = p.quota_semantics.unwrap();
        assert_ne!(semantics.status, SemanticsStatus::Known);
        assert!(
            semantics
                .effective_availability
                .iter()
                .all(|av| av.status == AvailabilityStatus::Unknown
                    && av.effective_percent_remaining.is_none())
        );
        assert!(p.windows.iter().all(|w| {
            matches!(
                w.pace.as_ref(),
                Some(QuotaPace {
                    status: QuotaPaceStatus::Unknown,
                    reason: Some(crate::types::QuotaPaceReason::Stale),
                    ..
                })
            )
        }));
    }

    #[test]
    fn claude_model_effective_headroom() {
        let mut p = provider(
            ProviderId::Claude,
            vec![
                QuotaWindow {
                    window_seconds: Some(18_000.0),
                    resets_at: Some(weekly_resets_at(0.2)),
                    ..window("five_hour", WindowKind::Session, 91.0)
                },
                QuotaWindow {
                    window_seconds: Some(WEEK_SECONDS),
                    resets_at: Some(weekly_resets_at(0.2)),
                    ..window("seven_day", WindowKind::Weekly, 3.0)
                },
                QuotaWindow {
                    window_seconds: Some(WEEK_SECONDS),
                    resets_at: Some(weekly_resets_at(0.2)),
                    ..window("model:fable", WindowKind::Model, 19.0)
                },
            ],
        );
        with_quota_semantics(&mut p, GENERATED_AT);
        let semantics = p.quota_semantics.unwrap();
        assert_eq!(semantics.status, SemanticsStatus::Known);

        let all = &semantics.effective_availability[0];
        assert_eq!(all.scope, "all_models");
        assert_eq!(all.status, AvailabilityStatus::Known);
        assert_eq!(all.effective_percent_remaining, Some(3.0));
        assert_eq!(all.bounded_by, vec!["five_hour", "seven_day"]);
        assert_eq!(all.limiting_window_ids, Some(vec!["seven_day".to_string()]));

        let model = &semantics.effective_availability[1];
        assert_eq!(model.scope, "model:fable");
        assert_eq!(model.effective_percent_remaining, Some(3.0));
        assert_eq!(
            model.bounded_by,
            vec!["five_hour", "seven_day", "model:fable"]
        );
    }

    #[test]
    fn codex_reports_bound_conflict_over_exhaustion() {
        let mut p = provider(
            ProviderId::Codex,
            vec![
                QuotaWindow {
                    starts_at: Some(iso(generated_at_ms())),
                    resets_at: Some(iso(generated_at_ms() + 5.0 * 60.0 * 60.0 * 1000.0)),
                    ..window("five_hour", WindowKind::Session, 92.0)
                },
                QuotaWindow {
                    starts_at: Some(iso(generated_at_ms() - 4.0 * 24.0 * 60.0 * 60.0 * 1000.0)),
                    resets_at: Some(iso(generated_at_ms() + 3.0 * 24.0 * 60.0 * 60.0 * 1000.0)),
                    ..window("weekly", WindowKind::Weekly, 0.0)
                },
                QuotaWindow {
                    starts_at: Some(iso(generated_at_ms())),
                    resets_at: Some(iso(generated_at_ms() + 5.0 * 60.0 * 60.0 * 1000.0)),
                    ..window("model:codex_bengalfox:5h", WindowKind::Model, 92.0)
                },
                QuotaWindow {
                    starts_at: Some(iso(generated_at_ms())),
                    resets_at: Some(iso(generated_at_ms() + 7.0 * 24.0 * 60.0 * 60.0 * 1000.0)),
                    ..window("model:codex_bengalfox:7d", WindowKind::Model, 96.0)
                },
            ],
        );
        with_quota_semantics(&mut p, GENERATED_AT);
        let semantics = p.quota_semantics.unwrap();

        let model = semantics
            .effective_availability
            .iter()
            .find(|av| av.scope == "model:codex_bengalfox")
            .unwrap();
        assert_eq!(model.status, AvailabilityStatus::Unknown);
        assert_eq!(
            model.bounded_by,
            vec![
                "five_hour",
                "weekly",
                "model:codex_bengalfox:5h",
                "model:codex_bengalfox:7d"
            ]
        );
        let conflict = model.bound_conflict.as_ref().unwrap();
        assert_eq!(conflict.exhausted_window_ids, vec!["weekly"]);
        assert_eq!(
            conflict.live_window_ids,
            vec!["model:codex_bengalfox:5h", "model:codex_bengalfox:7d"]
        );
        assert!(model.effective_percent_remaining.is_none());
        assert_eq!(model.runway.as_ref().unwrap().status, RunwayStatus::Unknown);
        assert_eq!(
            model.selection.as_ref().unwrap().status,
            SelectionStatus::Unknown
        );

        let all_models = semantics
            .effective_availability
            .iter()
            .find(|av| av.scope == "all_models")
            .unwrap();
        assert_eq!(all_models.status, AvailabilityStatus::Known);
        assert_eq!(all_models.effective_percent_remaining, Some(0.0));
        assert_eq!(
            all_models.runway.as_ref().unwrap().status,
            RunwayStatus::ExhaustedNow
        );
    }
}
