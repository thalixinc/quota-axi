//! Version + self-update for the `quota-axi` binary (GitHub releases, the #368 impl).

use crate::error::{Error, Result};
use crate::toon;
use std::cmp::Ordering;
use std::io::{IsTerminal, Write};
use std::process::Command;

pub const REPO: &str = "https://github.com/thalixinc/quota-axi";
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// The GitHub API endpoint `update` reads to learn the latest version — the `releases/latest`
/// OBJECT, never the git-tags feed. A tag-only release is invisible here, which keeps the
/// signal aligned with the cargo `--tag v<latest>` install.
pub const LATEST_RELEASE_ENDPOINT: &str = "repos/thalixinc/quota-axi/releases/latest";

/// Run the `cargo install --git --tag v<latest> --force` update. The FULL tag (leading `v`) is
/// required by cargo, unlike the stripped `latest` used for semver comparison + display.
fn do_update(latest: &str) -> Result<()> {
    let tag = format!("v{latest}");
    let status = Command::new("cargo")
        .args(["install", "--git", REPO, "--tag", &tag, "--force"])
        .status()
        .map_err(|e| Error::operational(format!("`cargo` not available: {e}"), "UPDATE"))?;
    if !status.success() {
        return Err(Error::operational("cargo install failed", "UPDATE").with_suggestions(vec![
            format!("Run `cargo install --git {REPO} --tag {tag} --force` manually."),
        ]));
    }
    Ok(())
}

/// `quota-axi version [--yes]`: always print the version line first (script-safe), then the
/// update-available surface (#368). A newer release → "update available"; `--yes` auto-updates;
/// otherwise an interactive `[y/N]` (non-tty = no, reported, never blocks). The availability
/// fetch is non-fatal: offline / rate-limited / no releases all degrade to the bare version
/// line (exit 0), never a hard error.
pub fn cmd_version(yes: bool) -> Result<()> {
    println!("quota-axi {VERSION}");

    let latest = match fetch_latest_release().unwrap_or(None) {
        Some(v) if semver_cmp(&v, VERSION) == Ordering::Greater => Some(v),
        _ => None,
    };
    let Some(latest) = latest else {
        return Ok(());
    };

    if yes {
        do_update(&latest)?;
        println!("update: quota-axi upgraded {VERSION} -> {latest}");
        return Ok(());
    }

    if !std::io::stdin().is_terminal() {
        println!(
            "update available: {latest} — run `quota-axi update`, or `quota-axi version --yes` to update now"
        );
        return Ok(());
    }
    print!("update available: {latest} — run quota-axi update, or --yes to update now [y/N] ");
    std::io::stdout().flush().ok();
    let mut line = String::new();
    let _ = std::io::stdin().read_line(&mut line);
    if matches!(line.trim(), "y" | "Y" | "yes" | "YES") {
        do_update(&latest)?;
        println!("update: quota-axi upgraded {VERSION} -> {latest}");
    }
    Ok(())
}

/// Compare two dotted semvers. Missing components count as 0.
pub fn semver_cmp(a: &str, b: &str) -> Ordering {
    let pa: Vec<u64> = a.split('.').map(|s| s.parse().unwrap_or(0)).collect();
    let pb: Vec<u64> = b.split('.').map(|s| s.parse().unwrap_or(0)).collect();
    for i in 0..pa.len().max(pb.len()) {
        match pa
            .get(i)
            .copied()
            .unwrap_or(0)
            .cmp(&pb.get(i).copied().unwrap_or(0))
        {
            Ordering::Equal => continue,
            other => return other,
        }
    }
    Ordering::Equal
}

/// Fetch the latest release tag (leading `v` stripped) via the authenticated `gh api` call
/// (handles private repos). `Ok(None)` = no release published.
fn fetch_latest_release() -> Result<Option<String>> {
    let out = Command::new("gh")
        .args(["api", LATEST_RELEASE_ENDPOINT, "--jq", ".tag_name"])
        .output()
        .map_err(|e| Error::operational(format!("`gh` not available: {e}"), "UPDATE_CHECK"))?;

    if !out.status.success() {
        // gh exits non-zero on errors or 404 (no releases).
        let stderr = String::from_utf8_lossy(&out.stderr);
        if stderr.contains("404") || stderr.contains("No releases found") {
            return Ok(None); // no releases published yet
        }
        return Err(Error::operational(
            "could not fetch the release feed (check `gh` auth and network)",
            "UPDATE_CHECK",
        )
        .with_suggestions(vec!["Run `gh auth login` to authenticate with GitHub.".into()]));
    }

    let stdout = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if stdout.is_empty() {
        return Ok(None); // no releases published yet
    }
    Ok(Some(stdout.trim_start_matches('v').to_string()))
}

/// `quota-axi update [--check] [--json]`: install the latest release (cargo), or report the
/// current-vs-latest comparison with `--check`.
pub fn cmd_update(check: bool, json: bool) -> Result<()> {
    let latest = fetch_latest_release()?;

    let (latest, available) = match latest {
        Some(v) => {
            let avail = semver_cmp(&v, VERSION) == Ordering::Greater;
            (v, avail)
        }
        None => {
            if check {
                if json {
                    println!(
                        "{}",
                        serde_json::json!({"package": "quota-axi", "current": VERSION, "latest": null, "available": false})
                    );
                } else {
                    println!(
                        "{}",
                        toon::join(&[
                            format!(
                                "update:\n  package: quota-axi\n  current: {VERSION}\n  latest: (none published)\n  available: false"
                            ),
                            toon::help_list(&["No GitHub releases published yet; the current build is the latest known.".into()]),
                        ])
                    );
                }
            } else if json {
                println!(
                    "{}",
                    serde_json::json!({"ok": true, "action": "update", "current": VERSION, "available": false})
                );
            } else {
                println!("ok: quota-axi already at latest ({VERSION}) — no published release");
            }
            return Ok(());
        }
    };

    if check {
        if json {
            println!(
                "{}",
                serde_json::json!({"package": "quota-axi", "current": VERSION, "latest": latest, "available": available})
            );
        } else {
            println!(
                "{}",
                toon::join(&[
                    format!(
                        "update:\n  package: quota-axi\n  current: {VERSION}\n  latest: {latest}\n  available: {available}"
                    ),
                    if available {
                        toon::help_list(&["Run `quota-axi update` to upgrade".to_string()])
                    } else {
                        toon::help_list(&["Already up to date".to_string()])
                    },
                ])
            );
        }
        return Ok(());
    }

    if !available {
        if json {
            println!(
                "{}",
                serde_json::json!({"ok": true, "action": "update", "current": VERSION, "latest": latest, "available": false})
            );
        } else {
            println!("ok: quota-axi already at latest ({VERSION})");
        }
        return Ok(());
    }

    do_update(&latest)?;

    if json {
        println!(
            "{}",
            serde_json::json!({"ok": true, "action": "update", "current": VERSION, "latest": latest, "available": true})
        );
    } else {
        println!("update: quota-axi upgraded {VERSION} -> {latest}");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn semver_ordering() {
        assert_eq!(semver_cmp("0.2.0", "0.1.0"), Ordering::Greater);
        assert_eq!(semver_cmp("0.1.0", "0.2.0"), Ordering::Less);
        assert_eq!(semver_cmp("0.1.0", "0.1.0"), Ordering::Equal);
        assert_eq!(semver_cmp("0.1.0", "0.1.0-rc1"), Ordering::Equal); // prerelease suffix dropped
        assert_eq!(semver_cmp("1.0.0", "1.0"), Ordering::Equal); // missing -> 0
        assert_eq!(semver_cmp("0.1.41", "0.1.42"), Ordering::Less);
    }

    /// The update path keys off the RELEASES feed (releases/latest), never the tags feed.
    #[test]
    fn update_reads_releases_latest_not_tags() {
        assert_eq!(
            LATEST_RELEASE_ENDPOINT,
            "repos/thalixinc/quota-axi/releases/latest"
        );
        assert!(LATEST_RELEASE_ENDPOINT.contains("releases/latest"));
    }
}
