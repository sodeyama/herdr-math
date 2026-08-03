//! Directory allowlist for shell auto-watch (`tmath agent-enable` /
//! `agent-disable` / `agent-allowed`).
//!
//! The shell wrapper installed by `scripts/install.sh` calls
//! `tmath agent-allowed` on every wrapped coding-agent launch, so the check
//! must stay silent and cheap; directory registration is a separate,
//! explicit, user-run step.

use std::env;
use std::fs;
use std::io::Write as _;
use std::path::{Path, PathBuf};

fn home_dir() -> Result<PathBuf, String> {
    env::var_os("HOME")
        .map(PathBuf::from)
        .ok_or_else(|| "HOME is not set".to_string())
}

fn allowlist_path() -> Result<PathBuf, String> {
    let base = match env::var_os("XDG_CONFIG_HOME") {
        Some(value) => PathBuf::from(value),
        None => home_dir()?.join(".config"),
    };
    Ok(base.join("tmath").join("agent-allowlist"))
}

fn target_dir(args: &[String]) -> Result<PathBuf, String> {
    let raw = match args.first() {
        Some(value) => PathBuf::from(value),
        None => env::current_dir().map_err(|error| format!("current directory: {error}"))?,
    };
    raw.canonicalize()
        .map_err(|error| format!("{}: {error}", raw.display()))
}

fn read_entries(path: &Path) -> Result<Vec<PathBuf>, String> {
    match fs::read_to_string(path) {
        Ok(text) => Ok(text
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty() && !line.starts_with('#'))
            .map(PathBuf::from)
            .collect()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(error) => Err(format!("{}: {error}", path.display())),
    }
}

fn write_entries(path: &Path, entries: &[PathBuf]) -> Result<(), String> {
    let parent = path
        .parent()
        .ok_or_else(|| "allowlist path has no parent directory".to_string())?;
    fs::create_dir_all(parent).map_err(|error| format!("{}: {error}", parent.display()))?;

    let mut text = String::new();
    for entry in entries {
        text.push_str(&entry.display().to_string());
        text.push('\n');
    }

    let mut open_options = fs::OpenOptions::new();
    open_options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        open_options.mode(0o600);
    }
    let mut file = open_options
        .open(path)
        .map_err(|error| format!("{}: {error}", path.display()))?;
    file.write_all(text.as_bytes())
        .map_err(|error| format!("{}: {error}", path.display()))
}

/// True when `dir` is `base` itself or a descendant of it, compared by path
/// component rather than string prefix (so `<base>2` never matches `<base>`).
fn is_within(base: &Path, dir: &Path) -> bool {
    dir.starts_with(base)
}

/// Adds `dir` to `entries` unless already present (exact match). Returns
/// whether the entry was newly added.
fn enable_entry(entries: &mut Vec<PathBuf>, dir: &Path) -> bool {
    if entries.iter().any(|entry| entry == dir) {
        return false;
    }
    entries.push(dir.to_path_buf());
    true
}

/// Removes `dir` from `entries` (exact match). Returns whether an entry was
/// removed.
fn disable_entry(entries: &mut Vec<PathBuf>, dir: &Path) -> bool {
    let before = entries.len();
    entries.retain(|entry| entry != dir);
    entries.len() != before
}

pub(crate) fn run_enable(args: &[String]) -> Result<i32, String> {
    if args.iter().any(|arg| arg == "--help" || arg == "-h") {
        println!("usage: tmath agent-enable [<dir>]");
        return Ok(0);
    }
    let dir = target_dir(args)?;
    let path = allowlist_path()?;
    let mut entries = read_entries(&path)?;
    if !enable_entry(&mut entries, &dir) {
        println!(
            "tmath: agent auto-watch already enabled for {}",
            dir.display()
        );
        return Ok(0);
    }
    write_entries(&path, &entries)?;
    println!("tmath: enabled agent auto-watch for {}", dir.display());
    Ok(0)
}

pub(crate) fn run_disable(args: &[String]) -> Result<i32, String> {
    if args.iter().any(|arg| arg == "--help" || arg == "-h") {
        println!("usage: tmath agent-disable [<dir>]");
        return Ok(0);
    }
    let dir = target_dir(args)?;
    let path = allowlist_path()?;
    let mut entries = read_entries(&path)?;
    if !disable_entry(&mut entries, &dir) {
        println!(
            "tmath: agent auto-watch was not enabled for {}",
            dir.display()
        );
        return Ok(0);
    }
    write_entries(&path, &entries)?;
    println!("tmath: disabled agent auto-watch for {}", dir.display());
    Ok(0)
}

pub(crate) fn run_allowed(args: &[String]) -> Result<i32, String> {
    // Silent by design: the shell wrapper calls this on every launch of a
    // wrapped coding-agent command, so stdout/stderr noise would leak into
    // every interactive prompt.
    let dir = match target_dir(args) {
        Ok(dir) => dir,
        Err(_) => return Ok(1),
    };
    let path = match allowlist_path() {
        Ok(path) => path,
        Err(_) => return Ok(1),
    };
    let entries = read_entries(&path).unwrap_or_default();
    if entries.iter().any(|base| is_within(base, &dir)) {
        Ok(0)
    } else {
        Ok(1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    static COUNTER: AtomicU64 = AtomicU64::new(0);

    struct TempHome {
        dir: PathBuf,
    }

    impl TempHome {
        fn new() -> Self {
            let id = COUNTER.fetch_add(1, Ordering::Relaxed);
            let dir = env::temp_dir().join(format!(
                "tmath-allowlist-test-{}-{}",
                std::process::id(),
                id
            ));
            fs::create_dir_all(&dir).unwrap();
            Self { dir }
        }

        fn path(&self) -> Result<PathBuf, String> {
            Ok(self.dir.join("agent-allowlist"))
        }

        fn subdir(&self, name: &str) -> PathBuf {
            let path = self.dir.join(name);
            fs::create_dir_all(&path).unwrap();
            path.canonicalize().unwrap()
        }
    }

    impl Drop for TempHome {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.dir);
        }
    }

    #[test]
    fn allowed_path_exact_match() {
        let home = TempHome::new();
        let path = home.path().unwrap();
        let target = home.subdir("proj");
        write_entries(&path, std::slice::from_ref(&target)).unwrap();

        let entries = read_entries(&path).unwrap();
        assert!(entries.iter().any(|base| is_within(base, &target)));
    }

    #[test]
    fn allowed_path_subdirectory() {
        let home = TempHome::new();
        let path = home.path().unwrap();
        let target = home.subdir("proj");
        let nested = target.join("src").join("deep");
        fs::create_dir_all(&nested).unwrap();
        write_entries(&path, std::slice::from_ref(&target)).unwrap();

        let entries = read_entries(&path).unwrap();
        assert!(entries.iter().any(|base| is_within(base, &nested)));
    }

    #[test]
    fn not_allowed_when_absent() {
        let home = TempHome::new();
        let path = home.path().unwrap();
        let target = home.subdir("proj");
        assert!(read_entries(&path).unwrap().is_empty());
        assert!(!read_entries(&path)
            .unwrap()
            .iter()
            .any(|base| is_within(base, &target)));
    }

    #[test]
    fn not_allowed_for_sibling_directory() {
        let home = TempHome::new();
        let path = home.path().unwrap();
        let target = home.subdir("proj");
        let sibling = home.subdir("proj2");
        write_entries(&path, &[target]).unwrap();

        let entries = read_entries(&path).unwrap();
        assert!(!entries.iter().any(|base| is_within(base, &sibling)));
    }

    #[test]
    fn enable_is_idempotent() {
        let target = PathBuf::from("/tmp/proj");
        let mut entries = Vec::new();
        assert!(enable_entry(&mut entries, &target));
        assert!(!enable_entry(&mut entries, &target));
        assert_eq!(entries, vec![target]);
    }

    #[test]
    fn disable_removes_only_target() {
        let a = PathBuf::from("/tmp/a");
        let b = PathBuf::from("/tmp/b");
        let mut entries = vec![a.clone(), b.clone()];
        assert!(disable_entry(&mut entries, &a));
        assert_eq!(entries, vec![b]);
    }

    #[test]
    fn disable_missing_entry_is_noop_success() {
        let mut entries: Vec<PathBuf> = vec![PathBuf::from("/tmp/a")];
        let before = entries.clone();
        assert!(!disable_entry(&mut entries, Path::new("/tmp/missing")));
        assert_eq!(entries, before);
    }

    #[test]
    fn is_within_rejects_string_prefix_without_component_boundary() {
        let base = Path::new("/tmp/proj");
        let sibling = Path::new("/tmp/proj2");
        assert!(!is_within(base, sibling));
        assert!(is_within(base, Path::new("/tmp/proj")));
        assert!(is_within(base, Path::new("/tmp/proj/src")));
    }
}
