//! Pace layer mirroring `src/pace.ts`: per-window cycle-average pace, effective
//! runway, aggregate pace summaries, and the cycle-weighted selection scalar.

use crate::types::{
    CycleBasis, EffectivePaceSummary, EffectiveRunway, EffectiveSelection, PaceSummaryStatus,
    ProjectionConfidence, QuotaPace, QuotaPaceReason, QuotaPaceStatus, QuotaWindow, RunwayStatus,
    SelectionStatus,
};
use chrono::{DateTime, SecondsFormat, Utc};

/// Reserve within this many percentage points of zero is treated as on_pace.
pub const PACE_ON_PACE_DEADBAND_PERCENT_POINTS: f64 = 1.0;

/// Linear exhaustion projections before this much of the cycle has elapsed are
/// labeled `early` rather than `established`.
pub const PACE_EARLY_ELAPSED_PERCENT: f64 = 10.0;

/// The selection scalar is reported within this symmetric bound.
pub const SELECTION_CLAMP_PERCENT_POINTS: f64 = 100.0;

/// Below this much remaining cycle time the selection ratio is dominated by the
/// four-decimal rounding of `time_remaining_percent` rather than by real signal,
/// so the window is treated as unmeasurable instead of producing a runaway or
/// infinite term.
pub const SELECTION_MIN_TIME_REMAINING_PERCENT: f64 = 0.01;

struct ResolvedCycle {
    cycle_seconds: f64,
    starts_at_ms: f64,
    resets_at_ms: f64,
    cycle_basis: CycleBasis,
}

pub fn compute_window_pace(
    window: &QuotaWindow,
    generated_at: &str,
    stale: bool,
) -> Option<QuotaPace> {
    if stale {
        return Some(unknown_pace(QuotaPaceReason::Stale));
    }

    let Some(generated_at_ms) = parse_js_date(generated_at) else {
        return Some(unknown_pace(QuotaPaceReason::InvalidCycle));
    };

    let Some(percent_remaining) = finite_number(window.percent_remaining) else {
        return Some(unknown_pace(QuotaPaceReason::MissingUsage));
    };
    let Some(percent_used) = resolve_percent_used(window, Some(percent_remaining)) else {
        return Some(unknown_pace(QuotaPaceReason::MissingUsage));
    };

    let cycle = match resolve_cycle(window, generated_at_ms) {
        Ok(cycle) => cycle,
        Err(reason) => return Some(unknown_pace(reason)),
    };

    let remaining_ms = cycle.resets_at_ms - generated_at_ms;
    let elapsed_ms = generated_at_ms - cycle.starts_at_ms;
    let time_remaining_percent = (100.0 * remaining_ms) / (cycle.cycle_seconds * 1000.0);
    let elapsed_percent = (100.0 * elapsed_ms) / (cycle.cycle_seconds * 1000.0);
    let reserve_percent_points = percent_remaining - time_remaining_percent;
    let status = classify_pace(reserve_percent_points);

    let mut pace = QuotaPace {
        status,
        reason: None,
        time_remaining_percent: Some(round_pace(time_remaining_percent)),
        elapsed_percent: Some(round_pace(elapsed_percent)),
        reserve_percent_points: Some(round_pace(reserve_percent_points)),
        burn_multiple: None,
        projected_exhausted_at: None,
        projection_confidence: None,
        cycle_basis: Some(cycle.cycle_basis),
        cycle_seconds: Some(cycle.cycle_seconds),
    };

    if elapsed_percent > 0.0 {
        pace.burn_multiple = Some(round_pace(percent_used / elapsed_percent));
    }

    if percent_used > 0.0 && elapsed_ms > 0.0 {
        let remaining_budget = percent_remaining;
        let burn_per_ms = percent_used / elapsed_ms;
        if burn_per_ms > 0.0 && remaining_budget >= 0.0 {
            let ms_to_exhaust = remaining_budget / burn_per_ms;
            let projected_exhausted_at_ms = generated_at_ms + ms_to_exhaust;
            if is_representable_date_ms(projected_exhausted_at_ms) {
                pace.projected_exhausted_at = Some(iso_from_ms(projected_exhausted_at_ms));
                pace.projection_confidence =
                    Some(if elapsed_percent < PACE_EARLY_ELAPSED_PERCENT {
                        ProjectionConfidence::Early
                    } else {
                        ProjectionConfidence::Established
                    });
            }
        }
    }

    Some(pace)
}

pub fn compute_effective_runway(windows: &[QuotaWindow], generated_at: &str) -> EffectiveRunway {
    let exhausted = windows
        .iter()
        .find(|w| finite_number(w.percent_remaining) == Some(0.0));
    let generated_at_ms = parse_js_date(generated_at);

    if let Some(exhausted) = exhausted {
        let mut runway = EffectiveRunway {
            status: RunwayStatus::ExhaustedNow,
            usable_runway_seconds: Some(0.0),
            projected_exhausted_at: None,
            limiting_window_id: Some(exhausted.id.clone()),
            projection_confidence: None,
            unmeasurable_window_ids: None,
        };
        if generated_at_ms.is_some_and(is_representable_date_ms) {
            runway.projected_exhausted_at = Some(iso_from_ms(generated_at_ms.unwrap()));
        }
        return runway;
    }

    if windows.is_empty() {
        return unknown_runway(windows);
    }
    let Some(generated_at_ms) = generated_at_ms else {
        return unknown_runway(windows);
    };
    if !is_representable_date_ms(generated_at_ms) {
        return unknown_runway(windows);
    }

    let mut unmeasurable_window_ids: Vec<String> = Vec::new();
    let mut projections: Vec<(usize, f64)> = Vec::new();
    let mut lowest_confidence: ProjectionConfidence = ProjectionConfidence::Established;

    for (idx, window) in windows.iter().enumerate() {
        let remaining = finite_number(window.percent_remaining);
        let pace = window.pace.as_ref();
        let resets_at = resolve_resets_at_outcome(window.resets_at.as_deref());
        let (resets_at_ms, is_missing, is_malformed) = match resets_at {
            ResetsAtOutcome::Missing => (None, true, false),
            ResetsAtOutcome::Malformed => (None, false, true),
            ResetsAtOutcome::Ok { ms } => (Some(ms), false, false),
        };

        if is_missing {
            if remaining.is_some() && is_zero_use(window, remaining.unwrap()) {
                continue;
            }
            unmeasurable_window_ids.push(window.id.clone());
            continue;
        }

        if remaining.is_none()
            || remaining.unwrap() < 0.0
            || remaining.unwrap() > 100.0
            || pace.is_none()
            || pace.unwrap().status == QuotaPaceStatus::Unknown
            || is_malformed
            || resets_at_ms.map_or(false, |ms| ms <= generated_at_ms)
        {
            unmeasurable_window_ids.push(window.id.clone());
            continue;
        }

        let remaining = remaining.unwrap();
        let pace = pace.unwrap();
        let resets_at_ms = resets_at_ms.unwrap();

        if is_zero_use(window, remaining) {
            if pace.elapsed_percent.unwrap_or(0.0) < PACE_EARLY_ELAPSED_PERCENT {
                lowest_confidence = ProjectionConfidence::Early;
            }
            continue;
        }

        let exhausted_at_ms = parse_timestamp(pace.projected_exhausted_at.as_deref());
        if exhausted_at_ms.is_none()
            || exhausted_at_ms.unwrap() <= generated_at_ms
            || pace.projection_confidence.is_none()
        {
            unmeasurable_window_ids.push(window.id.clone());
            continue;
        }
        let exhausted_at_ms = exhausted_at_ms.unwrap();
        if pace.projection_confidence == Some(ProjectionConfidence::Early) {
            lowest_confidence = ProjectionConfidence::Early;
        }
        if exhausted_at_ms < resets_at_ms {
            projections.push((idx, exhausted_at_ms));
        }
    }

    if !unmeasurable_window_ids.is_empty() {
        return EffectiveRunway {
            status: RunwayStatus::Unknown,
            usable_runway_seconds: None,
            projected_exhausted_at: None,
            limiting_window_id: None,
            projection_confidence: None,
            unmeasurable_window_ids: Some(unmeasurable_window_ids),
        };
    }

    if projections.is_empty() {
        return EffectiveRunway {
            status: RunwayStatus::ThroughReset,
            usable_runway_seconds: None,
            projected_exhausted_at: None,
            limiting_window_id: None,
            projection_confidence: Some(lowest_confidence),
            unmeasurable_window_ids: None,
        };
    }

    let &(limiting_idx, limiting_ms) = projections
        .iter()
        .min_by(|a, b| a.1.partial_cmp(&b.1).unwrap())
        .unwrap();

    EffectiveRunway {
        status: RunwayStatus::ProjectedExhaustion,
        usable_runway_seconds: Some(((limiting_ms - generated_at_ms) / 1000.0).round().max(0.0)),
        projected_exhausted_at: Some(iso_from_ms(limiting_ms)),
        limiting_window_id: Some(windows[limiting_idx].id.clone()),
        projection_confidence: windows[limiting_idx]
            .pace
            .as_ref()
            .and_then(|p| p.projection_confidence),
        unmeasurable_window_ids: None,
    }
}

pub fn summarize_effective_pace(windows: &[QuotaWindow]) -> EffectivePaceSummary {
    let mut ahead_window_ids: Vec<String> = Vec::new();
    let mut behind_window_ids: Vec<String> = Vec::new();
    let mut on_pace_window_ids: Vec<String> = Vec::new();
    let mut unknown_window_ids: Vec<String> = Vec::new();
    let mut worst_reserve_percent_points: Option<f64> = None;
    let mut worst_reserve_window_id: Option<String> = None;

    for window in windows {
        match window.pace.as_ref().map(|p| p.status) {
            Some(QuotaPaceStatus::Ahead) => ahead_window_ids.push(window.id.clone()),
            Some(QuotaPaceStatus::Behind) => behind_window_ids.push(window.id.clone()),
            Some(QuotaPaceStatus::OnPace) => on_pace_window_ids.push(window.id.clone()),
            _ => unknown_window_ids.push(window.id.clone()),
        }

        if let Some(reserve) = window.pace.as_ref().and_then(|p| p.reserve_percent_points) {
            if worst_reserve_percent_points.is_none()
                || reserve < worst_reserve_percent_points.unwrap()
            {
                worst_reserve_percent_points = Some(reserve);
                worst_reserve_window_id = Some(window.id.clone());
            }
        }
    }

    let status = aggregate_pace_status(
        ahead_window_ids.len(),
        behind_window_ids.len(),
        on_pace_window_ids.len(),
    );
    let mut summary = EffectivePaceSummary {
        status,
        ahead_window_ids: None,
        behind_window_ids: None,
        on_pace_window_ids: None,
        unknown_window_ids: None,
        worst_reserve_percent_points: None,
        worst_reserve_window_id: None,
    };
    if !ahead_window_ids.is_empty() {
        summary.ahead_window_ids = Some(ahead_window_ids);
    }
    if !behind_window_ids.is_empty() {
        summary.behind_window_ids = Some(behind_window_ids);
    }
    if !on_pace_window_ids.is_empty() {
        summary.on_pace_window_ids = Some(on_pace_window_ids);
    }
    if !unknown_window_ids.is_empty() {
        summary.unknown_window_ids = Some(unknown_window_ids);
    }
    if let (Some(reserve), Some(id)) = (worst_reserve_percent_points, worst_reserve_window_id) {
        summary.worst_reserve_percent_points = Some(reserve);
        summary.worst_reserve_window_id = Some(id);
    }
    summary
}

/// Cycle-weighted mean, across a scope's bounding windows, of the allowance each
/// window is projected to forfeit at reset if its observed burn continues:
///
///   gap_w       = percentRemaining_w / timeRemainingPercent_w - burnMultiple_w
///   scopeMetric = SUM(gap_w * cycleSeconds_w) / SUM(cycleSeconds_w)
///
/// `gap_w` is the per-window projected forfeiture `percentRemaining -
/// burnMultiple * timeRemainingPercent` divided by `timeRemainingPercent`, which
/// makes windows on different cycle clocks comparable. Positive means allowance
/// is on track to reach reset unused; `0` is exact utilization; negative means
/// the window is overdrawn against its reset clock.
///
/// Any bounding window without usable pace makes the whole scope unmeasurable:
/// an unknown window is never assumed healthy and never defaults to zero.
pub fn summarize_effective_selection(windows: &[QuotaWindow]) -> EffectiveSelection {
    if windows.is_empty() {
        return EffectiveSelection {
            status: SelectionStatus::Unknown,
            spend_priority: None,
            unmeasurable_window_ids: None,
        };
    }

    let mut unmeasurable_window_ids: Vec<String> = Vec::new();
    let mut weighted_gap_sum = 0.0;
    let mut cycle_seconds_sum = 0.0;

    for window in windows {
        let gap = window_selection_gap(window);
        let cycle_seconds = finite_number(window.pace.as_ref().and_then(|p| p.cycle_seconds));
        if gap.is_none() || cycle_seconds.is_none() || cycle_seconds.unwrap() <= 0.0 {
            unmeasurable_window_ids.push(window.id.clone());
            continue;
        }
        weighted_gap_sum += gap.unwrap() * cycle_seconds.unwrap();
        cycle_seconds_sum += cycle_seconds.unwrap();
    }

    if !unmeasurable_window_ids.is_empty() {
        return EffectiveSelection {
            status: SelectionStatus::Unknown,
            spend_priority: None,
            unmeasurable_window_ids: Some(unmeasurable_window_ids),
        };
    }
    let scope_metric = weighted_gap_sum / cycle_seconds_sum;
    if !scope_metric.is_finite() {
        return EffectiveSelection {
            status: SelectionStatus::Unknown,
            spend_priority: None,
            unmeasurable_window_ids: Some(windows.iter().map(|w| w.id.clone()).collect()),
        };
    }
    EffectiveSelection {
        status: SelectionStatus::Known,
        spend_priority: Some(round_pace(clamp(
            scope_metric,
            SELECTION_CLAMP_PERCENT_POINTS,
        ))),
        unmeasurable_window_ids: None,
    }
}

/// The per-window selection term, or `None` when the window is unmeasurable.
fn window_selection_gap(window: &QuotaWindow) -> Option<f64> {
    let pace = window.pace.as_ref()?;
    if pace.status == QuotaPaceStatus::Unknown {
        return None;
    }

    let percent_remaining = finite_number(window.percent_remaining)?;
    let time_remaining_percent = finite_number(pace.time_remaining_percent)?;
    if time_remaining_percent < SELECTION_MIN_TIME_REMAINING_PERCENT {
        return None;
    }

    let burn_multiple = resolve_selection_burn_multiple(window, percent_remaining)?;

    let gap = percent_remaining / time_remaining_percent - burn_multiple;
    if gap.is_finite() {
        Some(gap)
    } else {
        None
    }
}

/// `compute_window_pace` omits `burn_multiple` only when no cycle time has
/// elapsed yet. Nothing can have been consumed in zero elapsed time, so that
/// single zero-elapsed, zero-use case has an observed burn rate of 0 and keeps
/// the scope measurable. Any other absent `burn_multiple` is a real data gap.
fn resolve_selection_burn_multiple(window: &QuotaWindow, percent_remaining: f64) -> Option<f64> {
    let explicit = finite_number(window.pace.as_ref().and_then(|p| p.burn_multiple));
    if explicit.is_some() {
        return explicit;
    }
    let elapsed_percent = finite_number(window.pace.as_ref().and_then(|p| p.elapsed_percent));
    let percent_used = match finite_number(window.percent_used) {
        Some(value) => value,
        None => 100.0 - percent_remaining,
    };
    if elapsed_percent.is_none() || elapsed_percent.unwrap() > 0.0 || percent_used != 0.0 {
        return None;
    }
    Some(0.0)
}

fn clamp(value: f64, bound: f64) -> f64 {
    value.min(bound).max(-bound)
}

fn unknown_runway(windows: &[QuotaWindow]) -> EffectiveRunway {
    EffectiveRunway {
        status: RunwayStatus::Unknown,
        usable_runway_seconds: None,
        projected_exhausted_at: None,
        limiting_window_id: None,
        projection_confidence: None,
        unmeasurable_window_ids: if windows.is_empty() {
            None
        } else {
            Some(windows.iter().map(|w| w.id.clone()).collect())
        },
    }
}

fn is_zero_use(window: &QuotaWindow, percent_remaining: f64) -> bool {
    let percent_used = finite_number(window.percent_used);
    percent_remaining == 100.0 && (percent_used.is_none() || percent_used == Some(0.0))
}

fn resolve_cycle(window: &QuotaWindow, generated_at_ms: f64) -> Result<ResolvedCycle, QuotaPaceReason> {
    let Some(resets_at_ms) = parse_timestamp(window.resets_at.as_deref()) else {
        return Err(QuotaPaceReason::MissingCycle);
    };
    if resets_at_ms <= generated_at_ms {
        return Err(QuotaPaceReason::ExpiredReset);
    }

    let starts_at_ms = parse_timestamp(window.starts_at.as_deref());
    if let Some(starts_at_ms) = starts_at_ms {
        if starts_at_ms >= resets_at_ms {
            return Err(QuotaPaceReason::InvalidCycle);
        }
        if starts_at_ms > generated_at_ms {
            return Err(QuotaPaceReason::FutureCycleStart);
        }
        let cycle_seconds = (resets_at_ms - starts_at_ms) / 1000.0;
        if !(cycle_seconds > 0.0) || !cycle_seconds.is_finite() {
            return Err(QuotaPaceReason::InvalidCycle);
        }
        return Ok(ResolvedCycle {
            cycle_seconds,
            starts_at_ms,
            resets_at_ms,
            cycle_basis: CycleBasis::StartsAtResetsAt,
        });
    }

    let Some(window_seconds) = finite_number(window.window_seconds) else {
        return Err(QuotaPaceReason::MissingCycle);
    };
    if !(window_seconds > 0.0) {
        return Err(QuotaPaceReason::InvalidCycle);
    }

    let cycle_duration_ms = window_seconds * 1000.0;
    if !cycle_duration_ms.is_finite() {
        return Err(QuotaPaceReason::InvalidCycle);
    }
    let implied_starts_at_ms = resets_at_ms - cycle_duration_ms;
    if !is_representable_date_ms(implied_starts_at_ms) {
        return Err(QuotaPaceReason::InvalidCycle);
    }
    if implied_starts_at_ms > generated_at_ms {
        return Err(QuotaPaceReason::FutureCycleStart);
    }
    Ok(ResolvedCycle {
        cycle_seconds: window_seconds,
        starts_at_ms: implied_starts_at_ms,
        resets_at_ms,
        cycle_basis: CycleBasis::WindowSeconds,
    })
}

fn classify_pace(reserve_percent_points: f64) -> QuotaPaceStatus {
    if reserve_percent_points.abs() <= PACE_ON_PACE_DEADBAND_PERCENT_POINTS {
        QuotaPaceStatus::OnPace
    } else if reserve_percent_points < 0.0 {
        QuotaPaceStatus::Ahead
    } else {
        QuotaPaceStatus::Behind
    }
}

fn aggregate_pace_status(ahead: usize, behind: usize, on_pace: usize) -> PaceSummaryStatus {
    let known = ahead + behind + on_pace;
    if known == 0 {
        return PaceSummaryStatus::Unknown;
    }
    if ahead > 0 && behind > 0 {
        return PaceSummaryStatus::Mixed;
    }
    if ahead > 0 {
        return PaceSummaryStatus::Ahead;
    }
    if behind > 0 {
        return PaceSummaryStatus::Behind;
    }
    PaceSummaryStatus::OnPace
}

fn resolve_percent_used(window: &QuotaWindow, percent_remaining: Option<f64>) -> Option<f64> {
    let explicit = finite_number(window.percent_used);
    if explicit.is_some() {
        return explicit;
    }
    percent_remaining.map(|p| 100.0 - p)
}

fn unknown_pace(reason: QuotaPaceReason) -> QuotaPace {
    QuotaPace {
        status: QuotaPaceStatus::Unknown,
        reason: Some(reason),
        time_remaining_percent: None,
        elapsed_percent: None,
        reserve_percent_points: None,
        burn_multiple: None,
        projected_exhausted_at: None,
        projection_confidence: None,
        cycle_basis: None,
        cycle_seconds: None,
    }
}

fn parse_timestamp(value: Option<&str>) -> Option<f64> {
    let value = value?;
    if value.is_empty() {
        return None;
    }
    parse_js_date(value)
}

enum ResetsAtOutcome {
    Missing,
    Malformed,
    Ok { ms: f64 },
}

/// Distinguishes a genuinely absent `resetsAt` (the cycle has not been
/// triggered yet) from a present-but-unparseable one (a real data defect that
/// claims a reset it cannot honor). Effective runway treats only the former
/// as non-bounding.
fn resolve_resets_at_outcome(value: Option<&str>) -> ResetsAtOutcome {
    match value {
        None | Some("") => ResetsAtOutcome::Missing,
        Some(value) => match parse_js_date(value) {
            Some(ms) => ResetsAtOutcome::Ok { ms },
            None => ResetsAtOutcome::Malformed,
        },
    }
}

fn finite_number(value: Option<f64>) -> Option<f64> {
    value.filter(|v| v.is_finite())
}

fn is_representable_date_ms(value: f64) -> bool {
    value.is_finite() && value.abs() <= 8_640_000_000_000_000.0
}

fn round_pace(value: f64) -> f64 {
    (value * 10_000.0).round() / 10_000.0
}

fn parse_js_date(value: &str) -> Option<f64> {
    DateTime::parse_from_rfc3339(value)
        .ok()
        .map(|dt| dt.timestamp_millis() as f64)
}

fn iso_from_ms(ms: f64) -> String {
    DateTime::<Utc>::from_timestamp_millis(ms.trunc() as i64)
        .map(|dt| dt.to_rfc3339_opts(SecondsFormat::Millis, true))
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{
        CycleBasis, PaceSummaryStatus, ProjectionConfidence, QuotaPace, QuotaPaceReason,
        QuotaPaceStatus, QuotaWindow, RunwayStatus, SelectionStatus, WindowKind,
    };
    use chrono::{DateTime, SecondsFormat, Utc};

    const GENERATED_AT: &str = "2026-07-15T12:00:00.000Z";
    const WEEK_SECONDS: f64 = 604_800.0;
    const FIVE_HOURS_SECONDS: f64 = 18_000.0;

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

    fn resets_after(elapsed_fraction: f64, cycle_seconds: f64) -> String {
        iso(generated_at_ms() + cycle_seconds * (1.0 - elapsed_fraction) * 1000.0)
    }

    fn starts_before(elapsed_fraction: f64, cycle_seconds: f64) -> String {
        iso(generated_at_ms() - cycle_seconds * elapsed_fraction * 1000.0)
    }

    fn window(id: &str, percent_used: f64) -> QuotaWindow {
        QuotaWindow {
            id: id.to_string(),
            label: id.to_string(),
            kind: WindowKind::Weekly,
            percent_used: Some(percent_used),
            percent_remaining: Some(100.0 - percent_used),
            starts_at: None,
            resets_at: None,
            reset_text: None,
            window_seconds: None,
            spent_usd: None,
            limit_usd: None,
            pace: None,
        }
    }

    fn paced_window(id: &str, percent_remaining: f64, elapsed_fraction: f64) -> QuotaWindow {
        let mut w = QuotaWindow {
            window_seconds: Some(WEEK_SECONDS),
            resets_at: Some(resets_after(elapsed_fraction, WEEK_SECONDS)),
            ..window(id, 100.0 - percent_remaining)
        };
        w.pace = compute_window_pace(&w, GENERATED_AT, false);
        w
    }

    fn bounded(
        id: &str,
        percent_remaining: f64,
        time_remaining_percent: f64,
        burn_multiple: f64,
        cycle_seconds: f64,
    ) -> QuotaWindow {
        QuotaWindow {
            id: id.to_string(),
            label: id.to_string(),
            kind: WindowKind::Weekly,
            percent_used: Some(100.0 - percent_remaining),
            percent_remaining: Some(percent_remaining),
            starts_at: None,
            resets_at: None,
            reset_text: None,
            window_seconds: None,
            spent_usd: None,
            limit_usd: None,
            pace: Some(QuotaPace {
                status: QuotaPaceStatus::OnPace,
                reason: None,
                time_remaining_percent: Some(time_remaining_percent),
                elapsed_percent: Some(100.0 - time_remaining_percent),
                reserve_percent_points: None,
                burn_multiple: Some(burn_multiple),
                projected_exhausted_at: None,
                projection_confidence: None,
                cycle_basis: None,
                cycle_seconds: Some(cycle_seconds),
            }),
        }
    }

    #[test]
    fn classifies_ahead_behind_and_on_pace() {
        let ahead = compute_window_pace(
            &QuotaWindow {
                window_seconds: Some(WEEK_SECONDS),
                resets_at: Some(resets_after(0.25, WEEK_SECONDS)),
                ..window("weekly", 50.0)
            },
            GENERATED_AT,
            false,
        )
        .unwrap();
        assert_eq!(ahead.status, QuotaPaceStatus::Ahead);
        assert!((ahead.reserve_percent_points.unwrap() - (50.0 - 75.0)).abs() < 1e-4);
        assert!((ahead.burn_multiple.unwrap() - 2.0).abs() < 1e-4);
        assert_eq!(ahead.cycle_basis, Some(CycleBasis::WindowSeconds));

        let behind = compute_window_pace(
            &QuotaWindow {
                window_seconds: Some(WEEK_SECONDS),
                resets_at: Some(resets_after(0.5, WEEK_SECONDS)),
                ..window("weekly", 10.0)
            },
            GENERATED_AT,
            false,
        )
        .unwrap();
        assert_eq!(behind.status, QuotaPaceStatus::Behind);
        assert!((behind.reserve_percent_points.unwrap() - 40.0).abs() < 1e-4);
        assert!((behind.burn_multiple.unwrap() - 0.2).abs() < 1e-4);

        let on_pace = compute_window_pace(
            &QuotaWindow {
                window_seconds: Some(WEEK_SECONDS),
                resets_at: Some(resets_after(0.5, WEEK_SECONDS)),
                ..window("weekly", 50.0)
            },
            GENERATED_AT,
            false,
        )
        .unwrap();
        assert_eq!(on_pace.status, QuotaPaceStatus::OnPace);
        assert!(on_pace.reserve_percent_points.unwrap().abs() < 1e-4);
    }

    #[test]
    fn uses_starts_at_and_resets_at_cycle_basis() {
        let elapsed = 0.4;
        let pace = compute_window_pace(
            &QuotaWindow {
                kind: WindowKind::Credits,
                starts_at: Some(starts_before(elapsed, WEEK_SECONDS)),
                resets_at: Some(resets_after(elapsed, WEEK_SECONDS)),
                ..window("credits", 20.0)
            },
            GENERATED_AT,
            false,
        )
        .unwrap();
        assert_eq!(pace.status, QuotaPaceStatus::Behind);
        assert_eq!(pace.cycle_basis, Some(CycleBasis::StartsAtResetsAt));
        assert_eq!(pace.cycle_seconds, Some(WEEK_SECONDS));
        assert!((pace.elapsed_percent.unwrap() - 40.0).abs() < 1e-4);
        assert!((pace.reserve_percent_points.unwrap() - 20.0).abs() < 1e-4);
    }

    #[test]
    fn unknown_when_cycle_missing() {
        let pace = compute_window_pace(
            &QuotaWindow {
                resets_at: Some(resets_after(0.2, WEEK_SECONDS)),
                ..window("weekly", 10.0)
            },
            GENERATED_AT,
            false,
        )
        .unwrap();
        assert_eq!(pace.status, QuotaPaceStatus::Unknown);
        assert_eq!(pace.reason, Some(QuotaPaceReason::MissingCycle));
    }

    #[test]
    fn stale_yields_unknown_pace() {
        let pace = compute_window_pace(&window("weekly", 10.0), GENERATED_AT, true).unwrap();
        assert_eq!(pace.status, QuotaPaceStatus::Unknown);
        assert_eq!(pace.reason, Some(QuotaPaceReason::Stale));
    }

    #[test]
    fn runway_uses_earliest_projection() {
        let first = paced_window("weekly", 50.0, 0.25);
        let second = paced_window("five_hour", 50.0, 0.4);
        let runway = compute_effective_runway(&[first, second], GENERATED_AT);
        assert_eq!(runway.status, RunwayStatus::ProjectedExhaustion);
        assert_eq!(runway.usable_runway_seconds, Some(151_200.0));
        assert_eq!(runway.limiting_window_id.as_deref(), Some("weekly"));
        assert_eq!(
            runway.projection_confidence,
            Some(ProjectionConfidence::Established)
        );
    }

    #[test]
    fn runway_reports_exhausted_now_without_projection() {
        let runway = compute_effective_runway(&[window("weekly", 100.0)], GENERATED_AT);
        assert_eq!(runway.status, RunwayStatus::ExhaustedNow);
        assert_eq!(runway.usable_runway_seconds, Some(0.0));
        assert_eq!(runway.projected_exhausted_at.as_deref(), Some(GENERATED_AT));
        assert_eq!(runway.limiting_window_id.as_deref(), Some("weekly"));
    }

    #[test]
    fn selection_is_positive_zero_and_negative() {
        let positive = summarize_effective_selection(&[bounded("weekly", 80.0, 40.0, 1.0, WEEK_SECONDS)]);
        assert_eq!(positive.status, SelectionStatus::Known);
        assert_eq!(positive.spend_priority, Some(1.0));

        let exact = summarize_effective_selection(&[bounded("weekly", 50.0, 50.0, 1.0, WEEK_SECONDS)]);
        assert_eq!(exact.spend_priority, Some(0.0));

        let negative = summarize_effective_selection(&[bounded("weekly", 10.0, 50.0, 2.0, WEEK_SECONDS)]);
        assert_eq!(negative.status, SelectionStatus::Known);
        assert!((negative.spend_priority.unwrap() - (-1.8)).abs() < 1e-4);
    }

    #[test]
    fn selection_scalar_clamps_at_both_ends() {
        let positive = summarize_effective_selection(&[bounded("weekly", 100.0, 0.02, 0.0, WEEK_SECONDS)]);
        assert_eq!(positive.status, SelectionStatus::Known);
        assert_eq!(positive.spend_priority, Some(SELECTION_CLAMP_PERCENT_POINTS));

        let negative = summarize_effective_selection(&[bounded("weekly", 0.0, 50.0, 500.0, WEEK_SECONDS)]);
        assert_eq!(negative.status, SelectionStatus::Known);
        assert_eq!(negative.spend_priority, Some(-SELECTION_CLAMP_PERCENT_POINTS));
    }

    #[test]
    fn summarize_pace_flags_mixed_and_unknown() {
        let five_hour = QuotaWindow {
            pace: Some(QuotaPace {
                status: QuotaPaceStatus::Behind,
                reason: None,
                time_remaining_percent: None,
                elapsed_percent: None,
                reserve_percent_points: Some(20.0),
                burn_multiple: None,
                projected_exhausted_at: None,
                projection_confidence: None,
                cycle_basis: None,
                cycle_seconds: None,
            }),
            ..window("five_hour", 10.0)
        };
        let weekly = QuotaWindow {
            pace: Some(QuotaPace {
                status: QuotaPaceStatus::Ahead,
                reason: None,
                time_remaining_percent: None,
                elapsed_percent: None,
                reserve_percent_points: Some(-15.0),
                burn_multiple: None,
                projected_exhausted_at: None,
                projection_confidence: None,
                cycle_basis: None,
                cycle_seconds: None,
            }),
            ..window("weekly", 60.0)
        };

        let summary = summarize_effective_pace(&[five_hour, weekly]);
        assert_eq!(summary.status, PaceSummaryStatus::Mixed);
        assert_eq!(summary.ahead_window_ids, Some(vec!["weekly".to_string()]));
        assert_eq!(
            summary.behind_window_ids,
            Some(vec!["five_hour".to_string()])
        );
        assert_eq!(summary.worst_reserve_percent_points, Some(-15.0));
        assert_eq!(summary.worst_reserve_window_id.as_deref(), Some("weekly"));
    }
}
