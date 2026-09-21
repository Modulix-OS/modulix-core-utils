//! Per-user storage for the data the crate caches outside the system
//! configuration (downloaded metadata, generated files).

use std::fs;
use std::path::{Path, PathBuf};

use crate::mx;

/// Directory name reserved for Modulix inside the user's data directory.
const APP_DIR: &str = "mx";

/// Handle on one user's `~/.local/share/mx` tree, addressed with paths
/// relative to it.
///
/// # Fields
/// * `base_dir` - absolute path of the store's root, created on construction.
pub struct ConfigStore {
    base_dir: PathBuf,
}

impl ConfigStore {
    /// Opens the store of the user whose home directory is `home_dir`.
    ///
    /// # Parameters
    /// * `home_dir` - the user's home directory; `.local/share/mx` is appended
    ///   to it.
    ///
    /// # Post-conditions
    /// The root directory exists when this returns `Ok`, including any missing
    /// parent.
    ///
    /// # Errors
    /// [`mx::ErrorKind::IOError`] if the directory cannot be created (missing
    /// rights, or a non-directory in the way).
    pub fn new(home_dir: impl AsRef<Path>) -> mx::Result<Self> {
        let base_dir = home_dir.as_ref().join(".local/share").join(APP_DIR);

        fs::create_dir_all(&base_dir).map_err(mx::ErrorKind::IOError)?;

        Ok(Self { base_dir })
    }

    /// Resolves a relative path inside the store and prepares it for writing.
    ///
    /// # Parameters
    /// * `relative_path` - path relative to the store root; intermediate
    ///   directories may be absent.
    ///
    /// # Returns
    /// The absolute path, whose parent directory now exists. The file itself
    /// is not created and need not exist.
    ///
    /// # Errors
    /// [`mx::ErrorKind::IOError`] if the parent directories cannot be created.
    pub fn get(&self, relative_path: impl AsRef<Path>) -> mx::Result<PathBuf> {
        let path = self.base_dir.join(relative_path);

        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(mx::ErrorKind::IOError)?;
        }

        Ok(path)
    }

    /// Writes `data` to a file in the store, creating the parent directories.
    ///
    /// # Parameters
    /// * `relative_path` - destination, relative to the store root.
    /// * `data` - bytes to write.
    ///
    /// # Post-conditions
    /// The file is replaced wholesale; any previous content is lost. The write
    /// is not atomic - an interrupted call can leave a truncated file.
    ///
    /// # Errors
    /// [`mx::ErrorKind::IOError`] if the directories or the file cannot be
    /// written.
    pub fn save(&self, relative_path: impl AsRef<Path>, data: impl AsRef<[u8]>) -> mx::Result<()> {
        let path = self.get(relative_path)?;
        fs::write(path, data).map_err(mx::ErrorKind::IOError)
    }

    /// Reads a file from the store as raw bytes.
    ///
    /// # Parameters
    /// * `relative_path` - source, relative to the store root.
    ///
    /// # Returns
    /// The whole file content.
    ///
    /// # Errors
    /// [`mx::ErrorKind::IOError`], notably when the file does not exist.
    pub fn load(&self, relative_path: impl AsRef<Path>) -> mx::Result<Vec<u8>> {
        let path = self.base_dir.join(relative_path);
        fs::read(path).map_err(mx::ErrorKind::IOError)
    }

    /// Reads a file from the store as UTF-8 text.
    ///
    /// # Parameters
    /// * `relative_path` - source, relative to the store root.
    ///
    /// # Returns
    /// The whole file content.
    ///
    /// # Errors
    /// [`mx::ErrorKind::IOError`] when the file is missing, unreadable, or not
    /// valid UTF-8.
    pub fn load_string(&self, relative_path: impl AsRef<Path>) -> mx::Result<String> {
        let path = self.base_dir.join(relative_path);
        fs::read_to_string(path).map_err(mx::ErrorKind::IOError)
    }

    /// Tells whether a path exists in the store.
    ///
    /// # Parameters
    /// * `relative_path` - path to test, relative to the store root.
    ///
    /// # Returns
    /// `true` for any existing entry, file or directory; `false` also when the
    /// path cannot be stat'ed.
    pub fn exists(&self, relative_path: impl AsRef<Path>) -> bool {
        self.base_dir.join(relative_path).exists()
    }

    /// Deletes a file from the store.
    ///
    /// # Parameters
    /// * `relative_path` - file to delete, relative to the store root.
    ///
    /// # Errors
    /// [`mx::ErrorKind::IOError`] when the file is missing or is a directory;
    /// deleting an absent file is not silently accepted.
    pub fn remove(&self, relative_path: impl AsRef<Path>) -> mx::Result<()> {
        let path = self.base_dir.join(relative_path);
        fs::remove_file(path).map_err(mx::ErrorKind::IOError)
    }

    /// Root of the store, for callers that must hand an absolute path to an
    /// external tool.
    ///
    /// # Returns
    /// The directory chosen at construction, guaranteed to exist.
    pub fn get_path(&self) -> &PathBuf {
        &self.base_dir
    }
}
