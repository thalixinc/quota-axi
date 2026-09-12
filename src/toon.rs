//! Minimal TOON renderer matching `@toon-format/toon` (the Node quota-axi output surface):
//! strings are quoted only when they are not a safe unquoted token, and a string-list value
//! renders inline as `key[N]: v1,v2`. The update block additionally uses the list form the
//! `cf`/axi stack emits for prose help.

/// Whether a string can be encoded without quotes — the same rule `@toon-format/toon` applies.
fn is_safe_unquoted(value: &str) -> bool {
    if value.is_empty() {
        return false;
    }
    if value != value.trim() {
        return false;
    }
    if matches!(value, "true" | "false" | "null") {
        return false;
    }
    if is_numeric_like(value) {
        return false;
    }
    if value.contains(':') || value.contains('"') || value.contains('\\') {
        return false;
    }
    if value.contains(['[', ']', '{', '}']) {
        return false;
    }
    if value.contains(['\n', '\r', '\t']) {
        return false;
    }
    if value.contains(',') {
        return false;
    }
    if value.starts_with('-') {
        return false;
    }
    true
}

/// `/^-?\d+(?:\.\d+)?(?:e[+-]?\d+)?$/i` or `/^0\d+$/` — values that must be quoted as strings.
fn is_numeric_like(value: &str) -> bool {
    let v = value.to_ascii_lowercase();
    if v.starts_with('0') && v.len() > 1 && v[1..].chars().all(|c| c.is_ascii_digit()) {
        return true;
    }
    v.parse::<f64>().is_ok()
}

/// `\` -> `\\`, `"` -> `\"`, `\n`/`\r`/`\t` -> their two-char escapes.
fn escape_string(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n")
        .replace('\r', "\\r")
        .replace('\t', "\\t")
}

/// Encode a scalar string: unquoted when safe, otherwise a double-quoted, escaped literal.
pub fn encode_string(value: &str) -> String {
    if is_safe_unquoted(value) {
        value.to_string()
    } else {
        format!("\"{}\"", escape_string(value))
    }
}

/// The error surface (matches `@toon-format/toon`'s `{ error, code, help: [...] }`):
/// `error: <value>`, `code: <value>`, then an inline `help[N]: v1,v2` when suggestions exist.
pub fn error(message: &str, code: &str, suggestions: &[String]) -> String {
    let mut out = format!(
        "error: {}\ncode: {}",
        encode_string(message),
        encode_string(code)
    );
    if !suggestions.is_empty() {
        out.push('\n');
        out.push_str(&help(suggestions));
    }
    out
}

/// Inline list form: `help[N]: v1,v2`.
pub fn help(lines: &[String]) -> String {
    let joined = lines
        .iter()
        .map(|l| encode_string(l))
        .collect::<Vec<_>>()
        .join(",");
    format!("help[{}]: {}", lines.len(), joined)
}

/// List form (the `cf` update block's prose help): `help[N]:\n  - v1\n  - v2`.
pub fn help_list(lines: &[String]) -> String {
    if lines.is_empty() {
        return String::new();
    }
    let mut out = format!("help[{}]:", lines.len());
    for l in lines {
        out.push('\n');
        out.push_str(&format!("  - {}", l));
    }
    out
}

/// Join non-empty blocks with a blank line.
pub fn join(blocks: &[String]) -> String {
    blocks
        .iter()
        .filter(|b| !b.is_empty())
        .cloned()
        .collect::<Vec<_>>()
        .join("\n\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quotes_only_unsafe_values() {
        assert_eq!(encode_string("VALIDATION_ERROR"), "VALIDATION_ERROR");
        assert_eq!(encode_string("Run `quota-axi --help`"), "Run `quota-axi --help`");
        assert_eq!(encode_string("unknown argument: --nope"), "\"unknown argument: --nope\"");
        assert_eq!(encode_string("true"), "\"true\"");
        assert_eq!(encode_string("a,b"), "\"a,b\"");
    }

    #[test]
    fn error_surface_matches_toon() {
        let rendered = error(
            "unknown argument: --nope",
            "VALIDATION_ERROR",
            &["Run `quota-axi --help` for supported commands and flags".to_string()],
        );
        assert_eq!(
            rendered,
            "error: \"unknown argument: --nope\"\ncode: VALIDATION_ERROR\nhelp[1]: Run `quota-axi --help` for supported commands and flags"
        );
    }
}
