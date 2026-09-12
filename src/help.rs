//! Top-level help. The surface grows as commands port: this slice ships `version`/`update`,
//! with `quota`/`auth`/`models` and the full flag set landing in follow-on tickets (epic #8).

pub fn help() -> String {
    "usage: quota-axi [version|update] [flags]\n\
     commands[2]:\n\
     \x20 version, update\n\
     output:\n\
     \x20 `version` prints the installed version and reports an available update from the GitHub release feed; `update` installs it (cargo install --git).\n\
     flags[5]:\n\
     \x20 --yes, --check, --json, --help, -v/--version\n\
     examples:\n\
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
        assert_eq!(n, 2);
        assert_eq!(listed, n, "the command list and its count disagree");
    }
}
