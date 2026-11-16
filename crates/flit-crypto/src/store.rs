//! Where the identity and the display name live between runs.
//!
//! The fingerprint is only useful if it does not change every launch, so the
//! secret has to be persisted. For M1 that is a single file under the user's
//! configuration directory, created with owner-only permissions. The display
//! name sits beside it, because it has to survive a restart too.
//!
//! This is a deliberate, documented limitation: the platform keychain is better
//! (it can refuse export on some systems) and lands in M7 hardening. Until
//! then, the file is readable by the user account, which is the same threat
//! model as the account itself.

use std::{
    fs::{self, OpenOptions},
    io::{ErrorKind, Write},
    path::{Path, PathBuf},
};

use crate::{CryptoError, Identity, clean_name, random_name};

/// First line of an identity file. Bumping this is how a future format change
/// stays recognizable instead of being parsed as garbage.
const MAGIC: &str = "flit-identity-v1";

/// Things that go wrong storing an identity.
#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    /// No configuration directory could be determined for this user.
    #[error("could not determine a configuration directory; pass --identity")]
    NoConfigDir,
    /// The file does not exist yet.
    #[error("no identity at {0}")]
    Missing(PathBuf),
    /// The file could not be read.
    #[error("could not read {path}: {source}")]
    Read {
        /// The file.
        path: PathBuf,
        /// The operating system's reason.
        #[source]
        source: std::io::Error,
    },
    /// The file could not be written.
    #[error("could not write {path}: {source}")]
    Write {
        /// The file.
        path: PathBuf,
        /// The operating system's reason.
        #[source]
        source: std::io::Error,
    },
    /// The file exists but is not a Flit identity.
    #[error("{0} is not a Flit identity file")]
    Malformed(PathBuf),
    /// Owner-only permissions could not be applied.
    #[error("could not restrict permissions on {path}: {source}")]
    Permissions {
        /// The file.
        path: PathBuf,
        /// The operating system's reason.
        #[source]
        source: std::io::Error,
    },
    /// The operating system refused to produce randomness.
    #[error(transparent)]
    Entropy(#[from] CryptoError),
    /// The name was blank, or longer than a peer list can show.
    #[error("a display name has to be {0} to {1} characters")]
    BadName(usize, usize),
}

/// An identity file at a known path.
#[derive(Debug, Clone)]
pub struct IdentityFile {
    path: PathBuf,
}

impl IdentityFile {
    /// Use `path` as the identity file.
    #[must_use]
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    /// The conventional location: `%APPDATA%\Flit\identity.key` on Windows,
    /// `$XDG_CONFIG_HOME/flit/identity.key` (or `~/.config/...`) elsewhere.
    pub fn default_path() -> Result<Self, StoreError> {
        let dir = if cfg!(windows) {
            std::env::var_os("APPDATA")
                .or_else(|| std::env::var_os("LOCALAPPDATA"))
                .map(PathBuf::from)
                .map(|p| p.join("Flit"))
        } else {
            std::env::var_os("XDG_CONFIG_HOME")
                .map(PathBuf::from)
                .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
                .map(|p| p.join("flit"))
        };
        dir.map(|d| Self::new(d.join("identity.key")))
            .ok_or(StoreError::NoConfigDir)
    }

    /// The file this handle points at.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Read the identity, or `None` when the file does not exist.
    pub fn load(&self) -> Result<Option<Identity>, StoreError> {
        let text = match fs::read_to_string(&self.path) {
            Ok(text) => text,
            Err(err) if err.kind() == ErrorKind::NotFound => return Ok(None),
            Err(source) => {
                return Err(StoreError::Read {
                    path: self.path.clone(),
                    source,
                });
            }
        };
        parse_identity(&self.path, &text).map(Some)
    }

    /// Read the identity, failing if there is none.
    pub fn load_required(&self) -> Result<Identity, StoreError> {
        self.load()?
            .ok_or_else(|| StoreError::Missing(self.path.clone()))
    }

    /// Read the identity, creating one on first run.
    pub fn load_or_create(&self) -> Result<Identity, StoreError> {
        if let Some(identity) = self.load()? {
            return Ok(identity);
        }

        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent).map_err(|source| StoreError::Write {
                path: parent.to_path_buf(),
                source,
            })?;
        }

        let identity = Identity::generate()?;
        let body = format!("{MAGIC}\n{}\n", hex::encode(identity.secret_bytes()));

        match open_private(&self.path) {
            Ok(mut file) => {
                file.write_all(body.as_bytes())
                    .map_err(|source| StoreError::Write {
                        path: self.path.clone(),
                        source,
                    })?;
                Ok(identity)
            }
            // Another process won the race. Use what it wrote.
            Err(err) if err.kind() == ErrorKind::AlreadyExists => self.load_required(),
            Err(source) => Err(StoreError::Write {
                path: self.path.clone(),
                source,
            }),
        }
    }

    /// Replace the identity file atomically.
    ///
    /// Writing beside the target and renaming means a crash leaves either the
    /// old identity or the new one, never half of either.
    pub fn save(&self, identity: &Identity) -> Result<(), StoreError> {
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent).map_err(|source| StoreError::Write {
                path: parent.to_path_buf(),
                source,
            })?;
        }

        let temporary = self.path.with_extension("key.tmp");
        let body = format!("{MAGIC}\n{}\n", hex::encode(identity.secret_bytes()));
        {
            let mut file = open_private(&temporary).map_err(|source| StoreError::Write {
                path: temporary.clone(),
                source,
            })?;
            file.write_all(body.as_bytes())
                .map_err(|source| StoreError::Write {
                    path: temporary.clone(),
                    source,
                })?;
            file.sync_all().map_err(|source| StoreError::Write {
                path: temporary.clone(),
                source,
            })?;
        }
        fs::rename(&temporary, &self.path).map_err(|source| StoreError::Write {
            path: self.path.clone(),
            source,
        })
    }
}

/// The display name at a known path, beside the identity.
///
/// A name is not secret and proves nothing; it is text the person chooses. It
/// lives next to the key so the desktop shell, the CLI and the phone announce
/// the same one.
#[derive(Debug, Clone)]
pub struct NameFile {
    path: PathBuf,
}

impl NameFile {
    /// Use `path` as the name file.
    #[must_use]
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    /// The name that belongs to `identity`: `name`, in the same folder.
    #[must_use]
    pub fn beside(identity: &IdentityFile) -> Self {
        Self::new(identity.path().with_file_name("name"))
    }

    /// The file this handle points at.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Read the name, or `None` when there is no usable one.
    ///
    /// A file containing only whitespace, or something longer than a peer list
    /// can show, reads as absent rather than as a name.
    pub fn load(&self) -> Result<Option<String>, StoreError> {
        let text = match fs::read_to_string(&self.path) {
            Ok(text) => text,
            Err(err) if err.kind() == ErrorKind::NotFound => return Ok(None),
            Err(source) => {
                return Err(StoreError::Read {
                    path: self.path.clone(),
                    source,
                });
            }
        };
        Ok(clean_name(&text))
    }

    /// Read the name, generating two fresh words on first run.
    pub fn load_or_create(&self) -> Result<String, StoreError> {
        if let Some(name) = self.load()? {
            return Ok(name);
        }
        let name = random_name()?;
        self.save(&name)
    }

    /// Replace the name atomically, and return the cleaned form that was kept.
    ///
    /// Refuses a blank name or one longer than [`MAX_NAME_CHARS`]; the caller
    /// keeps the old name rather than announcing an empty row.
    pub fn save(&self, name: &str) -> Result<String, StoreError> {
        let name = clean_name(name)
            .ok_or(StoreError::BadName(1, crate::MAX_NAME_CHARS))?;

        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent).map_err(|source| StoreError::Write {
                path: parent.to_path_buf(),
                source,
            })?;
        }

        let temporary = self.path.with_extension("tmp");
        fs::write(&temporary, format!("{name}\n")).map_err(|source| StoreError::Write {
            path: temporary.clone(),
            source,
        })?;
        fs::rename(&temporary, &self.path).map_err(|source| StoreError::Write {
            path: self.path.clone(),
            source,
        })?;
        Ok(name)
    }
}

/// Open a file that only the current user may read.
fn open_private(path: &Path) -> std::io::Result<fs::File> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);

    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }

    options.open(path)
}

/// Parse the two-line format, refusing anything that is not exactly that.
fn parse_identity(path: &Path, text: &str) -> Result<Identity, StoreError> {
    let mut lines = text.lines().filter(|line| !line.trim().is_empty());
    let magic = lines.next();
    let secret = lines.next();
    let trailing = lines.next();

    if magic != Some(MAGIC) || trailing.is_some() {
        return Err(StoreError::Malformed(path.to_path_buf()));
    }

    let secret = secret.ok_or_else(|| StoreError::Malformed(path.to_path_buf()))?;
    let bytes =
        hex::decode(secret.trim()).map_err(|_| StoreError::Malformed(path.to_path_buf()))?;
    let bytes: [u8; 32] = bytes
        .try_into()
        .map_err(|_| StoreError::Malformed(path.to_path_buf()))?;
    Ok(Identity::from_secret_bytes(bytes))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A unique scratch path without pulling in a temp-directory crate.
    fn scratch(name: &str) -> PathBuf {
        let unique = format!(
            "flit-store-test-{}-{}-{name}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        );
        std::env::temp_dir().join(unique).join("identity.key")
    }

    #[test]
    fn creates_then_reuses_the_same_identity() {
        let path = scratch("reuse");
        let store = IdentityFile::new(&path);

        let first = store.load_or_create().unwrap();
        let second = store.load_or_create().unwrap();
        assert_eq!(first.fingerprint(), second.fingerprint());

        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn a_missing_file_loads_as_none() {
        let store = IdentityFile::new(scratch("missing"));
        assert!(store.load().unwrap().is_none());
        assert!(matches!(store.load_required(), Err(StoreError::Missing(_))));
    }

    #[test]
    fn saving_replaces_the_identity_atomically() {
        let path = scratch("replace");
        let store = IdentityFile::new(&path);
        let first = Identity::from_secret_bytes([1u8; 32]);
        let second = Identity::from_secret_bytes([2u8; 32]);

        store.save(&first).unwrap();
        store.save(&second).unwrap();

        let loaded = store.load_required().unwrap();
        assert_eq!(loaded.fingerprint(), second.fingerprint());
        assert!(!path.with_extension("key.tmp").exists());

        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn refuses_files_that_are_not_ours() {
        let path = scratch("garbage");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        for text in [
            "",
            "hello",
            "flit-identity-v1",
            "flit-identity-v2\n00112233445566778899aabbccddeeff00112233445566778899aabbccddeeff\n",
            "flit-identity-v1\nnot-hex\n",
            "flit-identity-v1\n0011\n",
            "flit-identity-v1\n00112233445566778899aabbccddeeff00112233445566778899aabbccddeeff\nextra\n",
        ] {
            std::fs::write(&path, text).unwrap();
            assert!(
                matches!(
                    IdentityFile::new(&path).load(),
                    Err(StoreError::Malformed(_))
                ),
                "{text:?}"
            );
        }
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn a_name_is_created_once_then_reused() {
        let identity = IdentityFile::new(scratch("name-once"));
        let name = NameFile::beside(&identity);

        let first = name.load_or_create().unwrap();
        assert_eq!(first.split(' ').count(), 2, "{first}");
        assert_eq!(name.load_or_create().unwrap(), first);
        assert_eq!(name.load().unwrap().as_deref(), Some(first.as_str()));

        let _ = std::fs::remove_dir_all(identity.path().parent().unwrap());
    }

    #[test]
    fn a_saved_name_replaces_the_old_one() {
        let identity = IdentityFile::new(scratch("name-replace"));
        let name = NameFile::beside(&identity);

        assert_eq!(name.save("  Rhea's   laptop ").unwrap(), "Rhea's laptop");
        assert_eq!(name.load().unwrap().as_deref(), Some("Rhea's laptop"));

        assert!(matches!(name.save(" \t "), Err(StoreError::BadName(_, _))));
        assert!(matches!(
            name.save(&"a".repeat(crate::MAX_NAME_CHARS + 1)),
            Err(StoreError::BadName(_, _))
        ));
        // A refused name leaves the old one in place.
        assert_eq!(name.load().unwrap().as_deref(), Some("Rhea's laptop"));

        let _ = std::fs::remove_dir_all(identity.path().parent().unwrap());
    }

    #[test]
    fn an_unusable_name_file_reads_as_absent() {
        let identity = IdentityFile::new(scratch("name-garbage"));
        let name = NameFile::beside(&identity);
        std::fs::create_dir_all(identity.path().parent().unwrap()).unwrap();
        std::fs::write(name.path(), "   \n").unwrap();
        assert!(name.load().unwrap().is_none());
        let _ = std::fs::remove_dir_all(identity.path().parent().unwrap());
    }

    #[cfg(unix)]
    #[test]
    fn the_identity_file_is_owner_only() {
        use std::os::unix::fs::PermissionsExt;

        let path = scratch("mode");
        let store = IdentityFile::new(&path);
        store.load_or_create().unwrap();

        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "identity must not be group- or world-readable");

        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }
}
