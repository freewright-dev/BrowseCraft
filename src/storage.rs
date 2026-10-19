//! Where BrowseCraft keeps its own files (settings, bookmarks, the password
//! manager's settings), and how it writes them: in the per-user config
//! folder, never the working directory, and always by atomic replacement.

use std::io::Write;
use std::path::{Path, PathBuf};

/// The per-user BrowseCraft folder (`%APPDATA%\BrowseCraft`,
/// `~/Library/Application Support/BrowseCraft`, `$XDG_CONFIG_HOME/BrowseCraft`).
#[cfg(not(test))]
pub fn config_dir() -> Result<PathBuf, String> {
    let env_path = |key| {
        std::env::var_os(key)
            .map(PathBuf::from)
            .filter(|p| p.is_absolute())
    };
    #[cfg(windows)]
    let base = env_path("APPDATA");
    #[cfg(target_os = "macos")]
    let base = env_path("HOME").map(|p| p.join("Library/Application Support"));
    #[cfg(not(any(windows, target_os = "macos")))]
    let base = env_path("XDG_CONFIG_HOME").or_else(|| env_path("HOME").map(|p| p.join(".config")));
    base.map(|p| p.join("BrowseCraft"))
        .ok_or_else(|| "The per-user configuration folder is unavailable.".into())
}

#[cfg(test)]
pub fn config_dir() -> Result<PathBuf, String> {
    // Tests never touch the user's real settings or the working directory.
    std::thread_local! { static DIR: tempfile::TempDir = tempfile::tempdir().unwrap(); }
    Ok(DIR.with(|dir| dir.path().join("BrowseCraft")))
}

/// `name` inside the config folder.
pub fn config_file(name: &str) -> Result<PathBuf, String> {
    Ok(config_dir()?.join(name))
}

/// Writes a private file (one of `config_file`'s): creates its folder
/// owner-only, refuses a folder that is a symlink, then replaces the file
/// atomically.
pub fn write_private(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let dir = path
        .parent()
        .ok_or_else(|| std::io::Error::other("Missing settings directory"))?;
    private_dir(dir)?;
    replace_atomic(path, bytes)
}

/// Creates `dir` owner-only if needed and checks it is a real directory.
pub fn private_dir(dir: &Path) -> std::io::Result<()> {
    let mut builder = std::fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder.create(dir)?;
    let meta = std::fs::symlink_metadata(dir)?;
    if meta.file_type().is_symlink() || !meta.is_dir() {
        return Err(std::io::Error::other(
            "Settings directory must be a real directory",
        ));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

/// Replaces `path` with `bytes` in one step. Exclusive temporary creation +
/// atomic replacement replaces a planted destination symlink itself, never
/// opens/truncates its target, and a crash cannot leave a half-written file.
/// tempfile uses rename on Unix and MoveFileExW(REPLACE_EXISTING) on Windows.
pub fn replace_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let dir = match path.parent() {
        Some(dir) if !dir.as_os_str().is_empty() => dir,
        _ => Path::new("."),
    };
    let mut file = tempfile::NamedTempFile::new_in(dir)?;
    file.write_all(bytes)?;
    file.as_file().sync_all()?;
    file.persist(path).map_err(|e| e.error)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn atomic_replacement_does_not_truncate_alias_targets() {
        let dir = tempfile::tempdir().unwrap();
        let victim = dir.path().join("victim.txt");
        let path = dir.path().join("settings.txt");
        std::fs::write(&victim, "keep me").unwrap();
        std::fs::hard_link(&victim, &path).unwrap();
        write_private(&path, b"new").unwrap();
        assert_eq!(std::fs::read_to_string(&victim).unwrap(), "keep me");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "new");
        std::fs::remove_file(&path).unwrap();
        #[cfg(windows)]
        std::os::windows::fs::symlink_file(&victim, &path).unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(&victim, &path).unwrap();
        write_private(&path, b"new").unwrap();
        assert_eq!(std::fs::read_to_string(&victim).unwrap(), "keep me");
        assert!(
            !std::fs::symlink_metadata(&path)
                .unwrap()
                .file_type()
                .is_symlink()
        );
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 2);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
                0o600
            );
            assert_eq!(
                std::fs::metadata(dir.path()).unwrap().permissions().mode() & 0o777,
                0o700
            );
        }
    }
}
