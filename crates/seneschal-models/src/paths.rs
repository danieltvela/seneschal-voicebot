//! Model storage locations (issue #35).
//!
//! New downloads go to the standard per-OS data directory:
//!
//! - Linux: `~/.local/share/seneschal/models`
//! - macOS: `~/Library/Application Support/Seneschal/models`
//!
//! `SENESCHAL_MODELS_DIR` overrides the root (packaged installs and tests).
//! Existing installs keep their models under the daemon workdir (`models/`)
//! or `~/.seneschal/models`; those legacy roots are detected so the catalog
//! can report them as present without moving anything.

use std::path::{Component, Path, PathBuf};

#[cfg(target_os = "macos")]
const DATA_SUBDIR: &str = "Seneschal";
#[cfg(not(target_os = "macos"))]
const DATA_SUBDIR: &str = "seneschal";

/// Root directory for app-managed model downloads.
pub fn models_root() -> PathBuf {
    if let Some(dir) = std::env::var_os("SENESCHAL_MODELS_DIR")
        && !dir.is_empty()
    {
        return PathBuf::from(dir);
    }
    let base = dirs::data_dir().unwrap_or_else(|| PathBuf::from("."));
    base.join(DATA_SUBDIR).join("models")
}

/// Legacy model locations, most significant first: the daemon workdir's
/// `models/` (dev + installer default) and `~/.seneschal/models` (installer
/// home layout).
pub fn legacy_roots(workdir: &Path) -> Vec<PathBuf> {
    let mut roots = vec![workdir.join("models")];
    if let Some(home) = dirs::home_dir() {
        roots.push(home.join(".seneschal").join("models"));
    }
    roots
}

/// True when both paths refer to the same file or directory, tolerating
/// missing canonicalization, relative vs absolute forms and `..` segments.
pub fn same_path(a: &Path, b: &Path) -> bool {
    if a == b {
        return true;
    }
    match (std::fs::canonicalize(a), std::fs::canonicalize(b)) {
        (Ok(a), Ok(b)) => a == b,
        _ => normalize(a) == normalize(b),
    }
}

fn normalize(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn models_root_respects_override_and_platform_default() {
        temp_env::with_var(
            "SENESCHAL_MODELS_DIR",
            Some("/tmp/seneschal-test-models"),
            || {
                assert_eq!(models_root(), PathBuf::from("/tmp/seneschal-test-models"));
            },
        );
        temp_env::with_var("SENESCHAL_MODELS_DIR", None::<&str>, || {
            let root = models_root();
            let expected = dirs::data_dir()
                .map(|base| base.join(DATA_SUBDIR).join("models"))
                .unwrap_or_else(|| root.clone());
            assert_eq!(root, expected);
        });
    }

    #[test]
    fn legacy_roots_cover_workdir_and_home() {
        let roots = legacy_roots(Path::new("/repo"));
        assert_eq!(roots[0], PathBuf::from("/repo/models"));
        assert!(roots.len() >= 2);
        if let Some(home) = dirs::home_dir() {
            assert!(roots.contains(&home.join(".seneschal").join("models")));
        }
    }

    #[test]
    fn same_path_handles_relative_and_dots() {
        let dir = tempfile::TempDir::new().unwrap();
        let base = dir.path();
        std::fs::write(base.join("file.bin"), b"x").unwrap();
        assert!(same_path(&base.join("file.bin"), &base.join("file.bin")));
        assert!(same_path(
            &base.join("file.bin"),
            &base.join("./sub/../file.bin")
        ));
        assert!(!same_path(&base.join("file.bin"), &base.join("other.bin")));
    }
}
