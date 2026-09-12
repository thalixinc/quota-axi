//! Cache-path + private-file helpers mirroring `src/lib/fs.ts`.

use std::path::{Path, PathBuf};

/// `~/.cache/quota-axi/quotas.json`, or under `$XDG_CACHE_HOME/quota-axi/` when set.
pub fn cache_file_path() -> PathBuf {
    cache_dir_path().join("quotas.json")
}

fn cache_dir_path() -> PathBuf {
    if let Some(xdg) = std::env::var_os("XDG_CACHE_HOME") {
        if !xdg.is_empty() {
            return PathBuf::from(xdg).join("quota-axi");
        }
    }
    home_dir().join(".cache").join("quota-axi")
}

fn home_dir() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
}


/// Create the parent directory with mode 0700 (matches the Node `ensurePrivateParent`).
pub fn ensure_private_parent(file: &Path) {
    if let Some(parent) = file.parent() {
        let _ = create_dir_all_private(parent);
    }
}

#[cfg(unix)]
fn create_dir_all_private(dir: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::DirBuilderExt;
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(dir)
}

#[cfg(not(unix))]
fn create_dir_all_private(dir: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)
}

/// Read a JSON file, returning `None` when it is missing or malformed
/// (matches `readJsonFile`, which collapses every failure to `undefined`).
pub fn read_json_file(file: &Path) -> Option<serde_json::Value> {
    let raw = std::fs::read(file).ok()?;
    serde_json::from_slice(&raw).ok()
}
