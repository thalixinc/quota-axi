//! Top-level help. The surface grows as commands port: this slice ships `quota` (the implicit
//! default), `version`, and `update`; `auth`/`models`/`--tui` land in follow-on tickets (epic #8).

pub fn help() -> String {
    "usage: quota-axi [quota|version|update] [flags]\n\
     commands[3]:\n\
     \x20 (none)=quota, version, update\n\
     output:\n\
     \x20 Default TOON reports local quota evidence (Antigravity via loopback). `version` prints the installed version and an available update from the GitHub release feed; `update` installs it (cargo install --git).\n\
     flags[11]:\n\
     \x20 --provider <claude,codex,cursor,copilot,grok,kimi,zai,agy,alibaba,opencode-go>, --json, --full, --tui, --refresh <30s-24h>, --once, --allow-keychain-prompt, --no-credential-refresh, --yes, --check, --help, -v/--version\n\
     examples:\n\
     \x20 quota-axi\n\
     \x20 quota-axi --provider agy\n\
     \x20 quota-axi --json\n\
     \x20 quota-axi --full\n\
     \x20 quota-axi version\n\
     \x20 quota-axi version --yes\n\
     \x20 quota-axi update\n\
     \x20 quota-axi update --check"
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `commands[N]` is computed from the list — and the list on the next line has N entries.
    #[test]
    fn help_counts_its_own_commands() {
        let h = help();
        let mut lines = h.lines();
        lines.next(); // usage
        let count_line = lines.next().unwrap();
        let n: usize = count_line
            .trim_start_matches("commands[")
            .split(']')
            .next()
            .unwrap()
            .parse()
            .unwrap();
        let listed = lines.next().unwrap().trim().split(", ").count();
        assert_eq!(n, 3);
        assert_eq!(listed, n, "the command list and its count disagree");
    }
}
