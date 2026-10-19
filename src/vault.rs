//! The password vault: a KeePass database (`.kdbx`, KDBX4) that opens in
//! KeePassXC and the other KeePass apps.
//!
//! A vault is protected by one of two things, the user's choice when it is
//! created:
//!
//! * a **password**, or
//! * a **key file**: a file with 32 random bytes in the KeePass XML key-file
//!   format. Whoever has the vault file but not the key file cannot open it.
//!
//! This module only deals with the file and the credentials in it. The
//! `passwords` module owns the unlocked vault and the pages and prompts.

use std::io::Write;
use std::path::{Path, PathBuf};

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;
use keepass::config::{DatabaseConfig, KdfConfig};
use keepass::db::{Entry, Group, Value};
use keepass::{Database, DatabaseKey};
use secstr::SecStr;

/// A saved login as the UI sees it. The password is not part of it: it is
/// fetched on demand with [`Vault::password`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Credential {
    pub id: String,
    pub title: String,
    pub username: String,
    pub url: String,
}

/// How a new vault is protected.
pub enum NewProtection {
    Password(String),
    /// Creates a new random key file at this path (it must not exist).
    KeyFile(PathBuf),
}

/// How an existing vault is unlocked.
pub enum Unlock {
    Password(String),
    KeyFile(PathBuf),
}

/// An unlocked vault.
pub struct Vault {
    path: PathBuf,
    db: Database,
    key: DatabaseKey,
}

/// Key derivation settings. Strong by default (Argon2id, 64 MiB), cheap in
/// tests so they stay fast.
fn config() -> DatabaseConfig {
    let (memory, iterations) = if cfg!(test) {
        (1024 * 1024, 1)
    } else {
        (64 * 1024 * 1024, 8)
    };
    DatabaseConfig {
        kdf_config: KdfConfig::Argon2id {
            iterations,
            memory,
            parallelism: 2,
            version: argon2::Version::Version13,
        },
        ..DatabaseConfig::default()
    }
}

/// The text of a KeePass XML (version 1) key file holding `key`.
fn keyfile_xml(key: &[u8; 32]) -> String {
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<KeyFile>\n\t<Meta>\n\t\t<Version>1.00</Version>\n\t</Meta>\n\t<Key>\n\t\t<Data>{}</Data>\n\t</Key>\n</KeyFile>\n",
        BASE64.encode(key)
    )
}

/// Writes a new random key file. Refuses to overwrite an existing file.
fn write_new_keyfile(path: &Path) -> Result<Vec<u8>, String> {
    let mut key = [0u8; 32];
    getrandom::fill(&mut key).map_err(|_| "Could not get random bytes from the system.".to_string())?;
    let text = keyfile_xml(&key);
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|e| match e.kind() {
            std::io::ErrorKind::AlreadyExists => {
                "A file already exists at the key file path. Pick a new name.".to_string()
            }
            _ => format!("Could not create the key file: {e}"),
        })?;
    file.write_all(text.as_bytes())
        .and_then(|_| file.sync_all())
        .map_err(|e| format!("Could not write the key file: {e}"))?;
    Ok(text.into_bytes())
}

fn key_from_keyfile(bytes: &[u8]) -> Result<DatabaseKey, String> {
    let mut reader = bytes;
    DatabaseKey::new()
        .with_keyfile(&mut reader)
        .map_err(|e| format!("Could not read the key file: {e}"))
}

fn entry_text(entry: &Entry, field: &str) -> String {
    entry.get(field).unwrap_or("").to_string()
}

fn collect(group: &Group, out: &mut Vec<Credential>) {
    for entry in &group.entries {
        out.push(Credential {
            id: entry.uuid.to_string(),
            title: entry_text(entry, "Title"),
            username: entry_text(entry, "UserName"),
            url: entry_text(entry, "URL"),
        });
    }
    for child in &group.groups {
        collect(child, out);
    }
}

fn find<'a>(group: &'a Group, id: &str) -> Option<&'a Entry> {
    group
        .entries
        .iter()
        .find(|e| e.uuid.to_string() == id)
        .or_else(|| group.groups.iter().find_map(|g| find(g, id)))
}

fn find_mut<'a>(group: &'a mut Group, id: &str) -> Option<&'a mut Entry> {
    if let Some(i) = group.entries.iter().position(|e| e.uuid.to_string() == id) {
        return group.entries.get_mut(i);
    }
    group.groups.iter_mut().find_map(|g| find_mut(g, id))
}

fn remove_from(group: &mut Group, id: &str) -> bool {
    let before = group.entries.len();
    group.entries.retain(|e| e.uuid.to_string() != id);
    if group.entries.len() != before {
        return true;
    }
    group.groups.iter_mut().any(|g| remove_from(g, id))
}

/// The host of `url` without `www.`, lowercase. Empty if there is none.
pub fn host_key(url: &str) -> String {
    let with_scheme = if url.contains("://") {
        url.to_string()
    } else {
        format!("https://{url}")
    };
    url::Url::parse(&with_scheme)
        .ok()
        .and_then(|u| u.host_str().map(|h| h.to_ascii_lowercase()))
        .map(|h| h.strip_prefix("www.").map(str::to_string).unwrap_or(h))
        .unwrap_or_default()
}

/// Whether a login saved for `saved` may be used on `page`: the same host,
/// or one is a subdomain of the other (`login.example.com` / `example.com`).
pub fn same_site(saved: &str, page: &str) -> bool {
    let (a, b) = (host_key(saved), host_key(page));
    if a.is_empty() || b.is_empty() {
        return false;
    }
    a == b || a.ends_with(&format!(".{b}")) || b.ends_with(&format!(".{a}"))
}

impl Vault {
    /// Creates a new, empty vault file protected as asked. Never overwrites
    /// an existing vault.
    pub fn create(path: &Path, protection: NewProtection) -> Result<Vault, String> {
        if path.exists() {
            return Err("A file already exists at that path. Pick a new name, or open it instead.".into());
        }
        if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty())
            && !dir.is_dir()
        {
            return Err("The folder for the vault does not exist.".into());
        }
        let key = match protection {
            NewProtection::Password(password) => {
                if password.chars().count() < 8 {
                    return Err("Use a password of at least 8 characters.".into());
                }
                DatabaseKey::new().with_password(&password)
            }
            NewProtection::KeyFile(key_path) => {
                if key_path == path {
                    return Err("The key file must be a different file from the vault.".into());
                }
                let bytes = write_new_keyfile(&key_path)?;
                key_from_keyfile(&bytes)?
            }
        };
        let vault = Vault {
            path: path.to_path_buf(),
            db: Database::new(config()),
            key,
        };
        vault.save().inspect_err(|_| {
            // Do not leave a half-made vault behind.
            let _ = std::fs::remove_file(path);
        })?;
        Ok(vault)
    }

    /// Opens and decrypts a vault file.
    pub fn open(path: &Path, unlock: &Unlock) -> Result<Vault, String> {
        let key = match unlock {
            Unlock::Password(password) => DatabaseKey::new().with_password(password),
            Unlock::KeyFile(key_path) => {
                let bytes = std::fs::read(key_path)
                    .map_err(|e| format!("Could not read the key file: {e}"))?;
                key_from_keyfile(&bytes)?
            }
        };
        let mut file =
            std::fs::File::open(path).map_err(|e| format!("Could not open the vault file: {e}"))?;
        let db = Database::open(&mut file, key.clone()).map_err(|e| match e {
            keepass::error::DatabaseOpenError::Key(keepass::error::DatabaseKeyError::IncorrectKey) => {
                match unlock {
                    Unlock::Password(_) => "Wrong password.".to_string(),
                    Unlock::KeyFile(_) => "That key file does not open this vault.".to_string(),
                }
            }
            other => format!("Could not open the vault: {other}"),
        })?;
        Ok(Vault {
            path: path.to_path_buf(),
            db,
            key,
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Every login in the vault (in every group), sorted by title.
    pub fn credentials(&self) -> Vec<Credential> {
        let mut out = Vec::new();
        collect(&self.db.root, &mut out);
        out.sort_by_key(|c| (c.title.to_lowercase(), c.username.to_lowercase()));
        out
    }

    /// How many logins the vault holds (cheap: no copies, no sorting).
    pub fn count(&self) -> usize {
        fn count(group: &Group) -> usize {
            group.entries.len() + group.groups.iter().map(count).sum::<usize>()
        }
        count(&self.db.root)
    }

    /// The logins saved for the site of `page_url`.
    pub fn for_site(&self, page_url: &str) -> Vec<Credential> {
        self.credentials()
            .into_iter()
            .filter(|c| same_site(&c.url, page_url))
            .collect()
    }

    pub fn password(&self, id: &str) -> Option<String> {
        find(&self.db.root, id).map(|e| entry_text(e, "Password"))
    }

    /// Adds a login and writes the vault. Returns its id.
    pub fn add(&mut self, title: &str, url: &str, username: &str, password: &str) -> Result<String, String> {
        if password.is_empty() {
            return Err("A login needs a password.".into());
        }
        let mut entry = Entry::new();
        let title = if title.trim().is_empty() {
            host_key(url)
        } else {
            title.trim().to_string()
        };
        entry.fields.insert("Title".into(), Value::Unprotected(title));
        entry.fields.insert("URL".into(), Value::Unprotected(url.trim().to_string()));
        entry
            .fields
            .insert("UserName".into(), Value::Unprotected(username.to_string()));
        entry
            .fields
            .insert("Password".into(), Value::Protected(SecStr::from(password)));
        let id = entry.uuid.to_string();
        self.db.root.entries.push(entry);
        self.save().inspect_err(|_| {
            self.db.root.entries.pop();
        })?;
        Ok(id)
    }

    /// Changes a login's password and writes the vault.
    pub fn set_password(&mut self, id: &str, password: &str) -> Result<(), String> {
        if password.is_empty() {
            return Err("A login needs a password.".into());
        }
        let entry = find_mut(&mut self.db.root, id).ok_or("That login no longer exists.")?;
        entry.update_history();
        entry
            .fields
            .insert("Password".into(), Value::Protected(SecStr::from(password)));
        self.save()
    }

    /// Removes a login and writes the vault.
    pub fn delete(&mut self, id: &str) -> Result<(), String> {
        if !remove_from(&mut self.db.root, id) {
            return Err("That login no longer exists.".into());
        }
        self.save()
    }

    /// Encrypts and writes the vault. The file is replaced atomically, so a
    /// crash cannot leave a half-written vault.
    fn save(&self) -> Result<(), String> {
        let mut buffer = Vec::new();
        self.db
            .save(&mut buffer, self.key.clone())
            .map_err(|e| format!("Could not encrypt the vault: {e}"))?;
        // An exclusively created, randomly named temporary file: a planted
        // `<vault>.tmp` symlink cannot redirect the write.
        crate::storage::replace_atomic(&self.path, &buffer)
            .map_err(|e| format!("Could not write the vault: {e}"))
    }
}

/// A random password of `length` characters from letters, digits and a few
/// symbols. Uses the system's random number generator, without modulo bias.
pub fn generate_password(length: usize) -> String {
    const ALPHABET: &[u8] = b"abcdefghijkmnopqrstuvwxyzABCDEFGHJKLMNPQRSTUVWXYZ23456789!@#$%^&*-_=+?";
    let length = length.clamp(8, 128);
    let limit = 256 - (256 % ALPHABET.len());
    let mut out = String::with_capacity(length);
    let mut buf = [0u8; 64];
    while out.len() < length {
        if getrandom::fill(&mut buf).is_err() {
            break;
        }
        for &b in &buf {
            if (b as usize) < limit && out.len() < length {
                out.push(ALPHABET[b as usize % ALPHABET.len()] as char);
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("browsecraft-vault-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn password_vault_roundtrip() {
        let dir = temp_dir("pw");
        let path = dir.join("v.kdbx");
        let mut vault = Vault::create(&path, NewProtection::Password("correct horse".into())).unwrap();
        let id = vault.add("Example", "https://www.example.com/login", "alice", "s3cret!").unwrap();
        assert_eq!(vault.password(&id).as_deref(), Some("s3cret!"));

        let reopened = Vault::open(&path, &Unlock::Password("correct horse".into())).unwrap();
        let list = reopened.credentials();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].title, "Example");
        assert_eq!(list[0].username, "alice");
        assert_eq!(reopened.password(&list[0].id).as_deref(), Some("s3cret!"));

        assert_eq!(
            Vault::open(&path, &Unlock::Password("wrong password".into())).err().as_deref(),
            Some("Wrong password.")
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn keyfile_vault_roundtrip_and_wrong_keyfile() {
        let dir = temp_dir("kf");
        let path = dir.join("v.kdbx");
        let key = dir.join("v.keyx");
        let mut vault = Vault::create(&path, NewProtection::KeyFile(key.clone())).unwrap();
        assert!(key.is_file());
        vault.add("Site", "https://site.test/", "bob", "pw-1234").unwrap();

        let reopened = Vault::open(&path, &Unlock::KeyFile(key.clone())).unwrap();
        assert_eq!(reopened.credentials()[0].username, "bob");

        // A different key file does not open it, and neither does a password.
        let other = dir.join("other.keyx");
        write_new_keyfile(&other).unwrap();
        assert_eq!(
            Vault::open(&path, &Unlock::KeyFile(other)).err().as_deref(),
            Some("That key file does not open this vault.")
        );
        assert!(Vault::open(&path, &Unlock::Password("anything".into())).is_err());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn creating_never_overwrites() {
        let dir = temp_dir("over");
        let path = dir.join("v.kdbx");
        std::fs::write(&path, b"precious").unwrap();
        assert!(Vault::create(&path, NewProtection::Password("long enough pw".into())).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), b"precious");

        let key = dir.join("k.keyx");
        std::fs::write(&key, b"my key").unwrap();
        let fresh = dir.join("fresh.kdbx");
        assert!(Vault::create(&fresh, NewProtection::KeyFile(key.clone())).is_err());
        assert_eq!(std::fs::read(&key).unwrap(), b"my key");
        assert!(!fresh.exists(), "no half-made vault is left behind");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn rejects_weak_passwords_and_missing_folders() {
        let dir = temp_dir("weak");
        assert!(Vault::create(&dir.join("a.kdbx"), NewProtection::Password("short".into())).is_err());
        assert!(
            Vault::create(&dir.join("nope").join("a.kdbx"), NewProtection::Password("long enough pw".into()))
                .is_err()
        );
        assert!(Vault::create(&dir.join("a.kdbx"), NewProtection::KeyFile(dir.join("a.kdbx"))).is_err());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn update_and_delete_are_saved() {
        let dir = temp_dir("edit");
        let path = dir.join("v.kdbx");
        let pw = Unlock::Password("a good password".into());
        let mut vault = Vault::create(&path, NewProtection::Password("a good password".into())).unwrap();
        let a = vault.add("A", "https://a.test/", "u", "one").unwrap();
        let b = vault.add("B", "https://b.test/", "u", "two").unwrap();
        vault.set_password(&a, "uno").unwrap();
        vault.delete(&b).unwrap();
        assert!(vault.delete(&b).is_err());
        let again = Vault::open(&path, &pw).unwrap();
        assert_eq!(again.credentials().len(), 1);
        assert_eq!(again.password(&a).as_deref(), Some("uno"));
        assert!(vault.add("C", "https://c.test/", "u", "").is_err());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn logins_are_matched_by_site() {
        let dir = temp_dir("site");
        let mut vault = Vault::create(&dir.join("v.kdbx"), NewProtection::Password("a good password".into())).unwrap();
        vault.add("Ex", "https://example.com/", "me", "pw").unwrap();
        vault.add("Other", "https://other.org/", "me", "pw").unwrap();
        let hits = vault.for_site("https://login.example.com/signin?x=1");
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].title, "Ex");
        assert!(vault.for_site("https://evil-example.com/").is_empty());
        assert!(vault.for_site("about:blank").is_empty());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn same_site_rules() {
        assert!(same_site("https://www.example.com/a", "http://example.com/b"));
        assert!(same_site("https://a.example.com", "https://example.com"));
        assert!(!same_site("https://example.com", "https://notexample.com"));
        assert!(!same_site("", "https://example.com"));
        assert_eq!(host_key("Example.COM/path"), "example.com");
    }

    #[test]
    fn keyfile_is_the_keepass_xml_format() {
        let xml = keyfile_xml(&[7u8; 32]);
        assert!(xml.contains("<Version>1.00</Version>") && xml.contains("<Data>"));
        let key = key_from_keyfile(xml.as_bytes()).unwrap();
        assert!(!key.is_empty());
    }

    #[test]
    fn generated_passwords_are_sized_and_varied() {
        let a = generate_password(20);
        let b = generate_password(20);
        assert_eq!(a.chars().count(), 20);
        assert_ne!(a, b);
        assert_eq!(generate_password(1).chars().count(), 8, "minimum length");
        assert_eq!(generate_password(1000).chars().count(), 128, "maximum length");
        assert!(a.is_ascii());
    }
}
