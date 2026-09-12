//! Degraded-source classification mirroring `src/lib/source-attempts.ts`.

use crate::types::{DegradedSource, SourceAttempt};

/// A credential source that was not genuinely absent and did not yield a reading.
pub fn is_degraded_source_attempt(attempt: &SourceAttempt) -> bool {
    if let Some(degraded) = attempt.degraded {
        return degraded;
    }
    if attempt.status == crate::types::AttemptStatus::Failed {
        return true;
    }
    attempt.status == crate::types::AttemptStatus::Skipped && attempt.credential_present == Some(true)
}

/// The degraded sources behind a report, in consultation order, one entry per source.
pub fn degraded_sources(attempts: Option<&[SourceAttempt]>) -> Vec<DegradedSource> {
    let mut result: Vec<DegradedSource> = Vec::new();
    for attempt in attempts.unwrap_or(&[]) {
        if attempt.status == crate::types::AttemptStatus::Success {
            result.retain(|d| d.source != attempt.source);
            continue;
        }
        if !is_degraded_source_attempt(attempt) {
            continue;
        }
        if result.iter().any(|d| d.source == attempt.source) {
            continue;
        }
        result.push(DegradedSource {
            source: attempt.source.clone(),
            error: attempt.error.clone(),
        });
    }
    result
}
