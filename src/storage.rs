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

/// Reads at most `max` bytes of `path`; a longer file is refused rather than
/// read in full, so a huge or endless file (a device, a crafted list) cannot
/// stall startup or exhaust memory.
pub fn read_limited(path: &Path, max: u64) -> std::io::Result<Vec<u8>> {
    use std::io::Read;
    let mut bytes = Vec::new();
    std::fs::File::open(path)?.take(max + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > max {
        return Err(std::io::Error::other(format!(
            "{} is larger than {} KB",
            path.display(),
            max / 1024
        )));
    }
    Ok(bytes)
}

/// `read_limited` for a UTF-8 text file.
pub fn read_text_limited(path: &Path, max: u64) -> std::io::Result<String> {
    String::from_utf8(read_limited(path, max)?).map_err(std::io::Error::other)
}

/// Settings files BrowseCraft writes itself are a few kilobytes.
pub const MAX_SETTINGS_FILE: u64 = 1 << 20;

/// Writes `bytes` to a new file at `path` in one step, never replacing a
/// file that is already there: the bytes go to a temporary file that is then
/// moved into place without clobbering, so a file that appears at `path`
/// meanwhile is kept and the call fails with `AlreadyExists`. Nothing is left
/// behind on failure. With `owner_only`, only the current user can open the
/// file, from before its first byte is written (Unix: tempfile creates files
/// 0600; Windows: a protected access list).
pub fn create_new(path: &Path, bytes: &[u8], owner_only: bool) -> std::io::Result<()> {
    let dir = match path.parent() {
        Some(dir) if !dir.as_os_str().is_empty() => dir,
        _ => Path::new("."),
    };
    let mut file = tempfile::NamedTempFile::new_in(dir)?;
    #[cfg(windows)]
    if owner_only {
        current_user_only(file.path())?;
    }
    #[cfg(not(windows))]
    let _ = owner_only;
    file.write_all(bytes)?;
    file.as_file().sync_all()?;
    file.persist_noclobber(path).map_err(|e| e.error)?;
    Ok(())
}

/// Replaces the access list of `path` with one entry, full access for the
/// current user, and stops it inheriting from its folder (which may be
/// shared).
#[cfg(windows)]
fn current_user_only(path: &Path) -> std::io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use windows::Win32::Foundation::{GENERIC_ALL, HLOCAL, LocalFree};
    use windows::Win32::Security::Authorization::{
        EXPLICIT_ACCESS_W, SE_FILE_OBJECT, SET_ACCESS, SetEntriesInAclW, SetNamedSecurityInfoW,
        TRUSTEE_IS_NAME, TRUSTEE_IS_USER, TRUSTEE_W,
    };
    use windows::Win32::Security::{
        ACL, DACL_SECURITY_INFORMATION, NO_INHERITANCE, PROTECTED_DACL_SECURITY_INFORMATION,
    };
    use windows::core::{PCWSTR, PWSTR};

    // "CURRENT_USER" is the trustee name Windows reserves for the caller.
    let mut user: Vec<u16> = "CURRENT_USER".encode_utf16().chain([0]).collect();
    let access = EXPLICIT_ACCESS_W {
        grfAccessPermissions: GENERIC_ALL.0,
        grfAccessMode: SET_ACCESS,
        grfInheritance: NO_INHERITANCE,
        Trustee: TRUSTEE_W {
            TrusteeForm: TRUSTEE_IS_NAME,
            TrusteeType: TRUSTEE_IS_USER,
            ptstrName: PWSTR(user.as_mut_ptr()),
            ..Default::default()
        },
    };
    let name: Vec<u16> = path.as_os_str().encode_wide().chain([0]).collect();
    let mut acl: *mut ACL = std::ptr::null_mut();
    // SAFETY: `user` and `name` are NUL-terminated and outlive the calls;
    // `acl` is allocated by SetEntriesInAclW and freed with LocalFree.
    unsafe {
        SetEntriesInAclW(Some(&[access]), None, &mut acl).ok()?;
        let result = SetNamedSecurityInfoW(
            PCWSTR(name.as_ptr()),
            SE_FILE_OBJECT,
            DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION,
            None,
            None,
            Some(acl),
            None,
        );
        let _ = LocalFree(Some(HLOCAL(acl.cast())));
        result.ok()?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_are_limited() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("f.txt");
        std::fs::write(&path, "x".repeat(100)).unwrap();
        assert_eq!(read_text_limited(&path, 100).unwrap().len(), 100);
        assert!(read_text_limited(&path, 99).is_err());
        assert!(read_limited(&dir.path().join("missing"), 10).is_err());
    }

    #[test]
    fn creating_never_replaces_and_leaves_nothing_behind() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("key.keyx");
        create_new(&path, b"secret", true).unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"secret");
        let err = create_new(&path, b"other", false).unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::AlreadyExists);
        assert_eq!(std::fs::read(&path).unwrap(), b"secret");
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1, "no temporary file left");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(std::fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600);
        }
        #[cfg(windows)]
        {
            // One explicit entry and nothing inherited from the folder.
            let out = std::process::Command::new("icacls").arg(&path).output().unwrap();
            let text = String::from_utf8_lossy(&out.stdout);
            let entries = text.lines().filter(|l| l.contains(":(")).count();
            assert_eq!(entries, 1, "{text}");
            assert!(!text.contains("(I)"), "{text}");
        }
    }

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
