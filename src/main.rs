//! quota-axi — the AXI CLI. `quota` is the implicit default command; `version`/`update` are
//! the #368 self-update verbs. `auth`/`models`/`--tui` port in follow-on tickets (epic #8).

mod advice;
mod args;
mod cache;
mod commands;
mod error;
mod help;
mod interpretation;
mod lib;
mod pace;
mod providers;
mod render;
mod toon;
mod types;
mod version;

use clap::{Parser, Subcommand};
use error::{Error, Result};

const BIN: &str = "quota-axi";

#[derive(Parser)]
#[command(
    name = BIN,
    version,
    about = "Report local agent-provider quota windows and model quota evidence.",
    disable_version_flag = true,
    disable_help_flag = true
)]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    /// Report local agent-provider quota windows (the default command).
    Quota {
        /// Quota flags — parsed by the quota flag parser for exact error framing.
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
    /// Print the installed version and report an available update from the GitHub release feed.
    Version {
        /// Update now when a newer version is published (no prompt).
        #[arg(long)]
        yes: bool,
    },
    /// Pull the latest release via `cargo install --git`.
    Update {
        /// Report current vs latest without installing.
        #[arg(long)]
        check: bool,
        /// Emit JSON.
        #[arg(long)]
        json: bool,
    },
}

fn main() {
    // A closed pipe (`quota-axi --help | head -1`) ends quietly like any Unix tool.
    #[cfg(unix)]
    unsafe {
        libc::signal(libc::SIGPIPE, libc::SIG_DFL);
    }
    let args: Vec<String> = std::env::args().skip(1).collect();
    match run(&args) {
        Ok(code) => std::process::exit(code),
        Err(e) => {
            // quota-axi writes its error surface to stdout (matching the Node build).
            println!("{}", toon::error(&e.message, e.code, &e.suggestions));
            std::process::exit(e.exit_code());
        }
    }
}

fn run(args: &[String]) -> Result<i32> {
    // Help wins over every other flag, matching the Node `normalizeArgv` ordering.
    if args.iter().any(|a| a == "--help" || a == "-h") {
        println!("{}", help::help());
        return Ok(0);
    }
    // A bare version flag prints the bare version (script-safe; `-v`/`-V`/`--version`).
    if args.iter().any(|a| a == "-v" || a == "-V" || a == "--version") {
        println!("{}", version::VERSION);
        return Ok(0);
    }

    // Route a flag-first or bare invocation onto the implicit `quota` command.
    let normalized = normalize_argv(args);
    let cli = Cli::try_parse_from(std::iter::once(BIN.to_string()).chain(normalized.iter().cloned()))
        .map_err(|e| map_clap_error(e, &normalized))?;

    match cli.command {
        Some(Command::Quota { args }) => {
            let bin_path = std::env::args().next().unwrap_or_else(|| BIN.to_string());
            let (output, code) = commands::quota_command(&args, &bin_path)?;
            println!("{output}");
            Ok(code)
        }
        Some(Command::Version { yes }) => {
            version::cmd_version(yes)?;
            Ok(0)
        }
        Some(Command::Update { check, json }) => {
            version::cmd_update(check, json)?;
            Ok(0)
        }
        None => {
            println!("{}", help::help());
            Ok(0)
        }
    }
}

/// `quota` is the implicit default: a bare call or a flag-first call means "run quota"
/// (mirrors the Node `normalizeArgv`). `auth`/`models` are recognized and surface a
/// truthful not-yet-ported error rather than an unknown command.
fn normalize_argv(raw: &[String]) -> Vec<String> {
    if raw.is_empty() {
        return vec!["quota".to_string()];
    }
    let first = raw[0].as_str();
    if matches!(first, "quota" | "version" | "update" | "auth" | "models") {
        return raw.to_vec();
    }
    if first.starts_with('-') {
        let mut out = vec!["quota".to_string()];
        out.extend(raw.iter().cloned());
        return out;
    }
    raw.to_vec()
}

fn map_clap_error(e: clap::Error, args: &[String]) -> Error {
    use clap::error::ErrorKind;
    match e.kind() {
        ErrorKind::UnknownArgument => {
            let flag = args
                .iter()
                .find(|a| a.starts_with('-'))
                .cloned()
                .unwrap_or_default();
            Error::usage(format!("unknown argument: {flag}")).with_suggestions(vec![
                "Run `quota-axi --help` for supported commands and flags".into(),
            ])
        }
        ErrorKind::InvalidSubcommand => {
            let cmd = args.first().cloned().unwrap_or_default();
            Error::usage(format!("Unknown command: {cmd}"))
                .with_suggestions(vec!["Run `--help` to see available commands".into()])
        }
        _ => Error::usage(e.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(xs: &[&str]) -> Vec<String> {
        xs.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn normalize_routes_flag_first_to_quota() {
        assert_eq!(normalize_argv(&[]), v(&["quota"]));
        assert_eq!(normalize_argv(&v(&["--json"])), v(&["quota", "--json"]));
        assert_eq!(normalize_argv(&v(&["--provider", "agy"])), v(&["quota", "--provider", "agy"]));
        assert_eq!(normalize_argv(&v(&["version"])), v(&["version"]));
        assert_eq!(normalize_argv(&v(&["foo"])), v(&["foo"]));
    }

    #[test]
    fn version_flags_print_bare_version() {
        for flag in ["-v", "-V", "--version"] {
            assert_eq!(run(&v(&[flag])).unwrap(), 0);
        }
    }

    #[test]
    fn help_flag_is_not_an_error() {
        assert_eq!(run(&v(&["--help"])).unwrap(), 0);
        assert_eq!(run(&v(&["version", "--help"])).unwrap(), 0);
    }

    #[test]
    fn unknown_command_is_usage_error() {
        let e = run(&v(&["foo"])).unwrap_err();
        assert_eq!(e.message, "Unknown command: foo");
        assert!(e.usage);
    }

    #[test]
    fn version_update_flags_parse() {
        assert_eq!(run(&v(&["version"])).unwrap(), 0);
        assert_eq!(run(&v(&["version", "--yes"])).unwrap(), 0);
        assert_eq!(run(&v(&["update", "--check"])).unwrap(), 0);
    }
}
