//! quota-axi — the AXI CLI. This slice ships the `version`/`update` verbs and the bare
//! `-v`/`-V`/`--version` fast path; `quota`/`auth`/`models` and the full flag set port in
//! follow-on tickets (epic #8).

mod error;
mod help;
mod toon;
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
    if let Err(e) = run(&args) {
        // quota-axi writes its error surface to stdout (matching the Node build).
        println!("{}", toon::error(&e.message, e.code, &e.suggestions));
        std::process::exit(e.exit_code());
    }
}

fn run(args: &[String]) -> Result<()> {
    // Help wins over every other flag, matching the Node `normalizeArgv` ordering.
    if args.iter().any(|a| a == "--help" || a == "-h") {
        println!("{}", help::help());
        return Ok(());
    }
    // A bare version flag prints the bare version (script-safe; `-v`/`-V`/`--version`).
    if args.iter().any(|a| a == "-v" || a == "-V" || a == "--version") {
        println!("{}", version::VERSION);
        return Ok(());
    }

    // No command: show help (the implicit `quota` default arrives with the quota port).
    let Some(cmd) = args.first() else {
        println!("{}", help::help());
        return Ok(());
    };
    let rest = &args[1..];

    // Validate each command's flags up front so a typo gets the exact AXI error, then parse.
    match cmd.as_str() {
        "version" => check_args(rest, &[("--yes", false)])?,
        "update" => check_args(rest, &[("--check", false), ("--json", false)])?,
        other if other.starts_with('-') => {
            return Err(Error::usage(format!("unknown argument: {other}")).with_suggestions(
                vec!["Run `quota-axi --help` for supported commands and flags".into()],
            ));
        }
        other => {
            return Err(Error::usage(format!("Unknown command: {other}"))
                .with_suggestions(vec!["Run `--help` to see available commands".into()]));
        }
    }

    let cli = Cli::try_parse_from(std::iter::once(BIN.to_string()).chain(args.iter().cloned()))
        .map_err(|e| Error::usage(e.to_string()))?;

    match cli.command {
        Some(Command::Version { yes }) => version::cmd_version(yes),
        Some(Command::Update { check, json }) => version::cmd_update(check, json),
        None => {
            println!("{}", help::help());
            Ok(())
        }
    }
}

/// Reject an unknown flag or a stray positional for a command, with the exact AXI error.
fn check_args(rest: &[String], allowed: &[(&str, bool)]) -> Result<()> {
    let mut it = rest.iter();
    while let Some(a) = it.next() {
        let name = a.split('=').next().unwrap_or(a);
        if !a.starts_with('-') {
            return Err(Error::usage(format!("unknown argument: {a}"))
                .with_suggestions(vec![
                    "Run `quota-axi --help` for supported commands and flags".into(),
                ]));
        }
        match allowed.iter().find(|(f, _)| *f == name) {
            // A value flag consumes its next token; none exist in this slice, kept for parity.
            Some((_, true)) if !a.contains('=') => {
                it.next();
            }
            Some(_) => {}
            None => {
                return Err(Error::usage(format!("unknown argument: {name}"))
                    .with_suggestions(vec![
                        "Run `quota-axi --help` for supported commands and flags".into(),
                    ]));
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(xs: &[&str]) -> Vec<String> {
        xs.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn version_flags_print_bare_version() {
        for flag in ["-v", "-V", "--version"] {
            assert!(run(&v(&[flag])).is_ok());
        }
    }

    #[test]
    fn help_flag_is_not_an_error() {
        assert!(run(&v(&["--help"])).is_ok());
        assert!(run(&v(&["-h"])).is_ok());
        // help beats other flags (Node normalizeArgv ordering).
        assert!(run(&v(&["version", "--help"])).is_ok());
    }

    #[test]
    fn unknown_command_and_flag_are_usage_errors() {
        let e = run(&v(&["foo"])).unwrap_err();
        assert_eq!(e.message, "Unknown command: foo");
        assert!(e.usage);

        let e = run(&v(&["version", "--nope"])).unwrap_err();
        assert_eq!(e.message, "unknown argument: --nope");
        assert!(e.usage);

        let e = run(&v(&["--nope"])).unwrap_err();
        assert_eq!(e.message, "unknown argument: --nope");
    }

    #[test]
    fn version_update_flags_parse() {
        assert!(run(&v(&["version"])).is_ok());
        assert!(run(&v(&["version", "--yes"])).is_ok());
        assert!(run(&v(&["update"])).is_ok());
        assert!(run(&v(&["update", "--check"])).is_ok());
        assert!(run(&v(&["update", "--check", "--json"])).is_ok());
    }
}
