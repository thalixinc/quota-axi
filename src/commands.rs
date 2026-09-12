//! Command bodies mirroring `src/commands.ts` — the `quota` path (auth/models/tui in follow-on PRs).

use crate::advice;
use crate::args::parse_flags;
use crate::cache;
use crate::error::{Error, Result};
use crate::interpretation;
use crate::providers::{self, failed_provider};
use crate::render;
use crate::types::{ProviderId, ProviderOptions, ProviderQuota, ProviderStatus, QuotaAxiResponse};

/// `quota` (the implicit default command): fetch, interpret, cache, and render.
/// Returns `(output, exit_code)` — exit 1 when every requested provider failed, else 0.
pub fn quota_command(args: &[String], bin_path: &str) -> Result<(String, i32)> {
    let flags = parse_flags(args)?;
    let options = ProviderOptions {
        allow_keychain_prompt: flags.allow_keychain_prompt,
        refresh_credentials: !flags.no_credential_refresh,
    };
    if flags.tui {
        return Err(Error::usage("--tui is not yet supported in this Rust build (epic #8)")
            .with_suggestions(vec!["Run `quota-axi` without --tui for the report".into()]));
    }

    let response = load_quota(&flags.providers, &options);
    let all_failed = response.providers.iter().all(is_failed);
    let exit_code = if all_failed { 1 } else { 0 };

    let output = if flags.json {
        serde_json::to_string_pretty(&render::quota_json_report(&response, flags.full))
            .map_err(|e| Error::operational(format!("could not serialize report: {e}"), "RENDER"))?
    } else {
        render::render_quota_toon(
            &render::redacted_response(&response, flags.full),
            bin_path,
            flags.full,
        )
    };
    Ok((output, exit_code))
}

fn load_quota(providers: &[ProviderId], options: &ProviderOptions) -> QuotaAxiResponse {
    let response = fetch_quota(providers, options);
    // Cache refresh is best-effort: a failed write never fails the report.
    cache::write_cached_providers(&response.providers);
    response
}

pub fn fetch_quota(providers: &[ProviderId], options: &ProviderOptions) -> QuotaAxiResponse {
    let generated_at = crate::lib::time::now_iso();
    let mut results: Vec<ProviderQuota> = Vec::new();
    for &provider in providers {
        match providers::adapter_for(provider) {
            Some(adapter) => {
                let mut quota = adapter.fetch_quota(options);
                interpretation::with_quota_semantics(&mut quota, &generated_at);
                results.push(quota);
            }
            None => {
                // Truthful for a provider whose adapter has not landed yet in this Rust build.
                results.push(failed_provider(
                    provider,
                    label_for(provider),
                    ProviderStatus::Error,
                    &format!("provider {provider} not yet ported in this Rust build (epic #8)"),
                    Vec::new(),
                    None,
                    None,
                    None,
                ));
            }
        }
    }
    advice::annotate_quota_advice(QuotaAxiResponse {
        generated_at,
        schema_version: 5,
        providers: results,
        help: None,
    })
}

fn is_failed(provider: &ProviderQuota) -> bool {
    !matches!(
        provider.state.status,
        ProviderStatus::Fresh | ProviderStatus::Stale
    )
}

fn label_for(provider: ProviderId) -> &'static str {
    match provider {
        ProviderId::Claude => "Claude",
        ProviderId::Codex => "Codex",
        ProviderId::Cursor => "Cursor",
        ProviderId::Copilot => "GitHub Copilot",
        ProviderId::Grok => "Grok",
        ProviderId::Kimi => "Kimi",
        ProviderId::Zai => "Z.AI",
        ProviderId::Agy => "Antigravity",
        ProviderId::Alibaba => "Alibaba",
        ProviderId::OpencodeGo => "OpenCode Go",
    }
}
