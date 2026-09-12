//! Flag parsing mirroring `src/args.ts` — the shared flag surface for `quota`/`auth`
//! (and `models`). Command routing is owned by the CLI; this interprets the flags that follow.

use crate::error::{Error, Result};
use crate::types::ProviderId;

pub const MIN_REFRESH_SECONDS: u64 = 30;
pub const MAX_REFRESH_SECONDS: u64 = 86_400;

/// The providers the `models` evidence join draws from (the native model catalog).
pub const MODEL_CATALOG_PROVIDER_IDS: [ProviderId; 4] = [
    ProviderId::Claude,
    ProviderId::Codex,
    ProviderId::Grok,
    ProviderId::Kimi,
];

pub type IntelligenceBucket = String;
pub type ModelSortKey = String;

#[derive(Debug)]
pub struct QuotaFlags {
    pub providers: Vec<ProviderId>,
    pub json: bool,
    pub full: bool,
    pub tui: bool,
    pub allow_keychain_prompt: bool,
    pub no_credential_refresh: bool,
    pub refresh_seconds: Option<u64>,
    pub once: bool,
}

#[derive(Debug)]
pub struct ModelsFlags {
    pub quota: QuotaFlags,
    pub intelligence: Option<IntelligenceBucket>,
    pub sort: Option<ModelSortKey>,
}

/// Parse the flags shared by the `quota` and `auth` commands.
pub fn parse_flags(args: &[String]) -> Result<QuotaFlags> {
    let common = parse_common_flags(args, None)?;
    if common.intelligence.is_some() || common.sort.is_some() {
        return Err(Error::usage(
            "--intelligence and --sort are only supported by the models command",
        )
        .with_suggestions(vec![
            "Run `quota-axi models --help` for supported models flags".into(),
        ]));
    }
    Ok(common.quota)
}

/// Parse flags accepted by the `models` evidence-join command.
pub fn parse_models_flags(args: &[String]) -> Result<ModelsFlags> {
    let common = parse_common_flags(args, Some(&MODEL_CATALOG_PROVIDER_IDS))?;
    if common.quota.tui {
        return Err(Error::usage("--tui is only supported by the quota command")
            .with_suggestions(vec!["Run `quota-axi --tui` for the human quota report".into()]));
    }
    let unsupported = common
        .quota
        .providers
        .iter()
        .find(|p| !MODEL_CATALOG_PROVIDER_IDS.contains(p));
    if let Some(p) = unsupported {
        return Err(Error::usage(format!("models does not support provider: {p}"))
            .with_suggestions(vec![format!(
                "Supported model providers: {}",
                MODEL_CATALOG_PROVIDER_IDS
                    .iter()
                    .map(|p| p.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            )]));
    }
    Ok(ModelsFlags {
        quota: common.quota,
        intelligence: common.intelligence,
        sort: common.sort,
    })
}

struct CommonFlags {
    quota: QuotaFlags,
    intelligence: Option<IntelligenceBucket>,
    sort: Option<ModelSortKey>,
}

fn parse_common_flags(
    args: &[String],
    default_providers: Option<&[ProviderId]>,
) -> Result<CommonFlags> {
    let mut provider_value: Option<String> = None;
    let mut json = false;
    let mut full = false;
    let mut tui = false;
    let mut once = false;
    let mut refresh_seconds: Option<u64> = None;
    let mut allow_keychain_prompt = false;
    let mut no_credential_refresh = false;
    let mut intelligence: Option<IntelligenceBucket> = None;
    let mut sort: Option<ModelSortKey> = None;

    let mut index = 0;
    while index < args.len() {
        let arg = &args[index];
        if arg == "--" {
            index += 1;
            continue;
        }
        if arg == "--json" {
            json = true;
        } else if arg == "--full" {
            full = true;
        } else if arg == "--tui" {
            tui = true;
        } else if arg == "--once" {
            once = true;
        } else if arg == "--refresh" {
            refresh_seconds = Some(parse_refresh_value(args.get(index + 1).map(|s| s.as_str()))?);
            index += 1;
        } else if let Some(v) = arg.strip_prefix("--refresh=") {
            refresh_seconds = Some(parse_refresh_value(Some(v))?);
        } else if arg == "--allow-keychain-prompt" {
            allow_keychain_prompt = true;
        } else if arg == "--no-credential-refresh" {
            no_credential_refresh = true;
        } else if arg == "--intelligence" {
            intelligence = Some(parse_intelligence_value(
                args.get(index + 1).map(|s| s.as_str()),
                "--intelligence",
            )?);
            index += 1;
        } else if let Some(v) = arg.strip_prefix("--intelligence=") {
            intelligence = Some(parse_intelligence_value(Some(v), "--intelligence")?);
        } else if arg == "--sort" {
            sort = Some(parse_sort_value(args.get(index + 1).map(|s| s.as_str()))?);
            index += 1;
        } else if let Some(v) = arg.strip_prefix("--sort=") {
            sort = Some(parse_sort_value(Some(v))?);
        } else if arg == "--provider" {
            let value = args.get(index + 1).map(|s| s.as_str());
            if value.is_none() {
                return Err(Error::usage(
                    "--provider requires a comma-separated provider list",
                )
                .with_suggestions(vec!["Pass --provider=... if the value begins with --".into()]));
            }
            provider_value = Some(value.unwrap().to_string());
            index += 1;
        } else if let Some(v) = arg.strip_prefix("--provider=") {
            provider_value = Some(v.to_string());
        } else {
            return Err(Error::usage(format!("unknown argument: {arg}")).with_suggestions(vec![
                "Run `quota-axi --help` for supported commands and flags".into(),
            ]));
        }
        index += 1;
    }

    if tui && json {
        return Err(Error::usage("--tui and --json are mutually exclusive output modes")
            .with_suggestions(vec![
                "Run `quota-axi --tui` for the human report or `quota-axi --json` for machine output"
                    .into(),
            ]));
    }
    let live_only_flag = if refresh_seconds.is_some() {
        Some("--refresh")
    } else if once {
        Some("--once")
    } else {
        None
    };
    if let Some(flag) = live_only_flag {
        if !tui {
            return Err(Error::usage(format!("{flag} is only supported with --tui"))
                .with_suggestions(vec![
                    "Run `quota-axi --tui --refresh 5m` for the live human report".into(),
                ]));
        }
    }

    let providers = if provider_value.is_none() {
        if let Some(defaults) = default_providers {
            defaults.to_vec()
        } else {
            parse_provider_scope(None)?
        }
    } else {
        parse_provider_scope(provider_value.as_deref())?
    };

    Ok(CommonFlags {
        quota: QuotaFlags {
            providers,
            json,
            full,
            tui,
            allow_keychain_prompt,
            no_credential_refresh,
            refresh_seconds,
            once,
        },
        intelligence,
        sort,
    })
}

fn parse_intelligence_value(value: Option<&str>, flag: &str) -> Result<IntelligenceBucket> {
    if let Some(v) = value {
        if v == "high" || v == "medium" || v == "low" {
            return Ok(v.to_string());
        }
    }
    Err(Error::usage(format!("{flag} requires high, medium, or low"))
        .with_suggestions(vec!["Run `quota-axi models --help` for supported models flags".into()]))
}

/// Accept a whole-unit duration (`45s`, `5m`, `1h`) or bare seconds.
fn parse_refresh_value(value: Option<&str>) -> Result<u64> {
    let raw = value.unwrap_or("").trim();
    let (digits, multiplier) = if let Some(rest) = raw.strip_suffix('h') {
        (rest, 3600u64)
    } else if let Some(rest) = raw.strip_suffix('m') {
        (rest, 60u64)
    } else if let Some(rest) = raw.strip_suffix('s') {
        (rest, 1u64)
    } else {
        (raw, 1u64)
    };
    let number: u64 = match digits.parse() {
        Ok(n) if !digits.is_empty() && digits.chars().all(|c| c.is_ascii_digit()) && digits.len() <= 7 => n,
        _ => {
            return Err(Error::usage("--refresh requires a duration such as 30s, 5m, or 1h")
                .with_suggestions(vec!["Pass --refresh=... if the value begins with --".into()]));
        }
    };
    let seconds = number * multiplier;
    if seconds < MIN_REFRESH_SECONDS || seconds > MAX_REFRESH_SECONDS {
        return Err(Error::usage(format!(
            "--refresh must be between {MIN_REFRESH_SECONDS}s and {}h",
            MAX_REFRESH_SECONDS / 3600
        ))
        .with_suggestions(vec![
            "Provider quota windows do not move fast enough for tighter polling".into(),
        ]));
    }
    Ok(seconds)
}

fn parse_sort_value(value: Option<&str>) -> Result<ModelSortKey> {
    if value == Some("runway") {
        return Ok("runway".to_string());
    }
    Err(Error::usage("--sort requires a supported comparator")
        .with_suggestions(vec!["Supported sort keys: runway".into()]))
}

fn parse_provider_scope(value: Option<&str>) -> Result<Vec<ProviderId>> {
    let providers = match value {
        None => crate::types::PROVIDER_IDS.to_vec(),
        Some(v) => {
            let mut ids: Vec<ProviderId> = Vec::new();
            for item in v.split(',').map(|s| s.trim()).filter(|s| !s.is_empty()) {
                let Some(id) = ProviderId::from_str(item) else {
                    return Err(Error::usage(format!("unsupported provider: {item}"))
                        .with_suggestions(vec![format!(
                            "Supported providers: {}",
                            crate::types::PROVIDER_IDS
                                .iter()
                                .map(|p| p.as_str())
                                .collect::<Vec<_>>()
                                .join(", ")
                        )]));
                };
                if !ids.contains(&id) {
                    ids.push(id);
                }
            }
            ids
        }
    };
    Ok(providers)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(xs: &[&str]) -> Vec<String> {
        xs.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn parses_basic_flags() {
        let flags = parse_flags(&v(&["--json", "--full", "--provider", "claude,agy"])).unwrap();
        assert!(flags.json);
        assert!(flags.full);
        assert_eq!(
            flags.providers,
            vec![ProviderId::Claude, ProviderId::Agy]
        );
    }

    #[test]
    fn defaults_to_all_providers() {
        let flags = parse_flags(&v(&[])).unwrap();
        assert_eq!(flags.providers.len(), 10);
    }

    #[test]
    fn unknown_flag_is_usage_error() {
        let e = parse_flags(&v(&["--nope"])).unwrap_err();
        assert_eq!(e.message, "unknown argument: --nope");
        assert!(e.usage);
    }

    #[test]
    fn unsupported_provider() {
        let e = parse_flags(&v(&["--provider", "bogus"])).unwrap_err();
        assert_eq!(e.message, "unsupported provider: bogus");
    }

    #[test]
    fn tui_json_mutually_exclusive() {
        let e = parse_flags(&v(&["--tui", "--json"])).unwrap_err();
        assert!(e.message.contains("mutually exclusive"));
    }

    #[test]
    fn refresh_bounds() {
        assert!(parse_flags(&v(&["--tui", "--refresh", "5m"])).is_ok());
        let e = parse_flags(&v(&["--refresh", "5m"])).unwrap_err();
        assert!(e.message.contains("only supported with --tui"));
        let e = parse_flags(&v(&["--tui", "--refresh", "10s"])).unwrap_err();
        assert!(e.message.contains("must be between"));
    }

    #[test]
    fn models_rejects_tui() {
        let e = parse_models_flags(&v(&["--tui"])).unwrap_err();
        assert!(e.message.contains("--tui is only supported"));
    }
}
