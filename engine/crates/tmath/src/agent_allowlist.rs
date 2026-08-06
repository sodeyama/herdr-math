//! Directory allowlist for shell auto-watch (`tmath agent-enable` /
//! `agent-disable` / `agent-allowed`).
//!
//! Allowlist entries live in `[agent].allowlist` inside
//! `~/.config/tmath/config.toml`. The shell wrapper calls `tmath agent-allowed`
//! on every wrapped coding-agent launch, so the check must stay silent and cheap.

use std::env;
use std::path::PathBuf;

use crate::config;

fn target_dir(args: &[String]) -> Result<PathBuf, String> {
    let raw = match args.first() {
        Some(value) => PathBuf::from(value),
        None => env::current_dir().map_err(|error| format!("current directory: {error}"))?,
    };
    raw.canonicalize()
        .map_err(|error| format!("{}: {error}", raw.display()))
}

pub(crate) fn run_enable(args: &[String]) -> Result<i32, String> {
    if args.iter().any(|arg| arg == "--help" || arg == "-h") {
        println!("usage: tmath agent-enable [<dir>]");
        return Ok(0);
    }
    let dir = target_dir(args)?;
    if !config::enable_allowlist_dir(&dir)? {
        println!(
            "tmath: agent auto-watch already enabled for {}",
            dir.display()
        );
        return Ok(0);
    }
    println!("tmath: enabled agent auto-watch for {}", dir.display());
    Ok(0)
}

pub(crate) fn run_disable(args: &[String]) -> Result<i32, String> {
    if args.iter().any(|arg| arg == "--help" || arg == "-h") {
        println!("usage: tmath agent-disable [<dir>]");
        return Ok(0);
    }
    let dir = target_dir(args)?;
    if !config::disable_allowlist_dir(&dir)? {
        println!(
            "tmath: agent auto-watch was not enabled for {}",
            dir.display()
        );
        return Ok(0);
    }
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
    if config::is_dir_allowlisted(&dir) {
        Ok(0)
    } else {
        Ok(1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::sync::atomic::{AtomicU64, Ordering};

    static COUNTER: AtomicU64 = AtomicU64::new(0);

    use std::sync::{Mutex, MutexGuard};

    static TEST_ENV: Mutex<()> = Mutex::new(());

    struct TempHome {
        dir: PathBuf,
        _lock: MutexGuard<'static, ()>,
    }

    impl TempHome {
        fn new() -> Self {
            let lock = TEST_ENV.lock().unwrap();
            let id = COUNTER.fetch_add(1, Ordering::Relaxed);
            let dir = env::temp_dir().join(format!(
                "tmath-allowlist-test-{}-{}",
                std::process::id(),
                id
            ));
            fs::create_dir_all(&dir).unwrap();
            env::set_var("XDG_CONFIG_HOME", dir.join("xdg-config"));
            fs::create_dir_all(dir.join("xdg-config/tmath")).unwrap();
            Self { dir, _lock: lock }
        }

        fn subdir(&self, name: &str) -> PathBuf {
            let path = self.dir.join(name);
            fs::create_dir_all(&path).unwrap();
            path.canonicalize().unwrap()
        }
    }

    impl Drop for TempHome {
        fn drop(&mut self) {
            env::remove_var("XDG_CONFIG_HOME");
            let _ = fs::remove_dir_all(&self.dir);
        }
    }

    #[test]
    fn allowed_path_exact_match() {
        let _home = TempHome::new();
        let target = _home.subdir("proj");
        config::enable_allowlist_dir(&target).unwrap();
        assert!(config::is_dir_allowlisted(&target));
    }

    #[test]
    fn allowed_path_subdirectory() {
        let _home = TempHome::new();
        let target = _home.subdir("proj");
        let nested = target.join("src").join("deep");
        fs::create_dir_all(&nested).unwrap();
        config::enable_allowlist_dir(&target).unwrap();
        assert!(config::is_dir_allowlisted(&nested));
    }

    #[test]
    fn not_allowed_when_absent() {
        let _home = TempHome::new();
        let target = _home.subdir("proj");
        assert!(!config::is_dir_allowlisted(&target));
    }

    #[test]
    fn not_allowed_for_sibling_directory() {
        let _home = TempHome::new();
        let target = _home.subdir("proj");
        let sibling = _home.subdir("proj2");
        config::enable_allowlist_dir(&target).unwrap();
        assert!(!config::is_dir_allowlisted(&sibling));
    }

    #[test]
    fn enable_is_idempotent() {
        let _home = TempHome::new();
        let target = PathBuf::from("/tmp/proj");
        assert!(config::enable_allowlist_dir(&target).unwrap());
        assert!(!config::enable_allowlist_dir(&target).unwrap());
    }

    #[test]
    fn disable_removes_only_target() {
        let _home = TempHome::new();
        let a = PathBuf::from("/tmp/a");
        let b = PathBuf::from("/tmp/b");
        config::enable_allowlist_dir(&a).unwrap();
        config::enable_allowlist_dir(&b).unwrap();
        assert!(config::disable_allowlist_dir(&a).unwrap());
        assert!(config::is_dir_allowlisted(&b));
        assert!(!config::is_dir_allowlisted(&a));
    }

    #[test]
    fn disable_missing_entry_is_noop_success() {
        let _home = TempHome::new();
        assert!(!config::disable_allowlist_dir(&PathBuf::from("/tmp/missing")).unwrap());
    }
}
