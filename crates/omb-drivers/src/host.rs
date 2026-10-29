use std::collections::HashSet;
use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};
use std::time::Duration;

const HOME_DIRS: &[&str] = &[
    ".local/bin",
    ".npm-global/bin",
    ".claude/local",
    ".bun/bin",
    ".volta/bin",
    ".cargo/bin",
    ".deno/bin",
    ".asdf/shims",
    ".local/share/mise/shims",
    "bin",
];
const SYSTEM_DIRS: &[&str] = &["/usr/local/bin", "/opt/homebrew/bin"];

pub struct Host {
    pub home: PathBuf,
    pub path: OsString,
    pub codex_home: Option<OsString>,
    pub openai_api_key: bool,
    pub jev_api_key: bool,
    pub timeout: Duration,
}

impl Host {
    pub fn from_process() -> Self {
        let home = std::env::home_dir().unwrap_or_else(|| PathBuf::from("/"));
        let base = std::env::var_os("PATH").unwrap_or_default();
        Self {
            path: augmented_path(&base, &home),
            codex_home: std::env::var_os("CODEX_HOME"),
            openai_api_key: std::env::var_os("OPENAI_API_KEY").is_some_and(|k| !k.is_empty()),
            jev_api_key: false,
            timeout: Duration::from_secs(8),
            home,
        }
    }

    pub(crate) fn resolve(&self, name: &str) -> Option<PathBuf> {
        std::env::split_paths(&self.path).map(|dir| dir.join(name)).find(|p| is_executable(p))
    }
}

/// A GUI launch inherits a bare PATH, so the dirs agent CLIs install into are appended.
pub fn augmented_path(base: &OsStr, home: &Path) -> OsString {
    let mut dirs: Vec<PathBuf> = std::env::split_paths(base).collect();
    dirs.extend(HOME_DIRS.iter().map(|d| home.join(d)));
    dirs.extend(nvm_bin_dirs(home));
    dirs.extend(SYSTEM_DIRS.iter().map(PathBuf::from));
    let mut seen = HashSet::new();
    dirs.retain(|d| !d.as_os_str().is_empty() && seen.insert(d.clone()));
    std::env::join_paths(dirs).unwrap_or_else(|_| base.to_owned())
}

fn nvm_bin_dirs(home: &Path) -> Vec<PathBuf> {
    let base = home.join(".nvm/versions/node");
    let Ok(entries) = std::fs::read_dir(&base) else { return Vec::new() };
    let mut versions: Vec<(Vec<u64>, PathBuf)> = entries
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            let parts = name.strip_prefix('v')?.split('.').map(|p| p.parse().ok()).collect::<Option<Vec<u64>>>()?;
            Some((parts, e.path().join("bin")))
        })
        .collect();
    versions.sort_by(|a, b| b.0.cmp(&a.0));
    versions.into_iter().map(|(_, dir)| dir).collect()
}

#[cfg(unix)]
fn is_executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    path.metadata().is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
}

#[cfg(not(unix))]
fn is_executable(path: &Path) -> bool {
    path.is_file()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn appends_install_dirs_once_after_the_inherited_path() {
        let home = Path::new("/home/u");
        let path = augmented_path(OsStr::new("/usr/bin:/home/u/.local/bin"), home);
        let dirs: Vec<PathBuf> = std::env::split_paths(&path).collect();
        assert_eq!(dirs[0], Path::new("/usr/bin"));
        assert_eq!(dirs.iter().filter(|d| **d == Path::new("/home/u/.local/bin")).count(), 1);
        assert!(dirs.contains(&PathBuf::from("/home/u/.npm-global/bin")));
        assert!(dirs.contains(&PathBuf::from("/opt/homebrew/bin")));
    }

    #[test]
    fn nvm_versions_come_newest_first() {
        let tmp = tempfile::tempdir().unwrap();
        for v in ["v9.0.0", "v22.1.0", "v22.10.0"] {
            std::fs::create_dir_all(tmp.path().join(".nvm/versions/node").join(v)).unwrap();
        }
        let names: Vec<String> = nvm_bin_dirs(tmp.path())
            .iter()
            .map(|d| d.parent().unwrap().file_name().unwrap().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names, ["v22.10.0", "v22.1.0", "v9.0.0"]);
    }
}
