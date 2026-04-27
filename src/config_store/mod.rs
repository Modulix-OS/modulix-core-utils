use std::fs;
use std::path::{Path, PathBuf};

use crate::mx;

const APP_DIR: &str = "mx";

pub struct ConfigStore {
    base_dir: PathBuf,
}

impl ConfigStore {
    pub fn new(home_dir: impl AsRef<Path>) -> mx::Result<Self> {
        let base_dir = home_dir.as_ref().join(".local/share").join(APP_DIR);

        fs::create_dir_all(&base_dir).map_err(mx::ErrorKind::IOError)?;

        Ok(Self { base_dir })
    }

    pub fn get(&self, relative_path: impl AsRef<Path>) -> mx::Result<PathBuf> {
        let path = self.base_dir.join(relative_path);

        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(mx::ErrorKind::IOError)?;
        }

        Ok(path)
    }

    pub fn save(&self, relative_path: impl AsRef<Path>, data: impl AsRef<[u8]>) -> mx::Result<()> {
        let path = self.get(relative_path)?;
        fs::write(path, data).map_err(mx::ErrorKind::IOError)
    }

    pub fn load(&self, relative_path: impl AsRef<Path>) -> mx::Result<Vec<u8>> {
        let path = self.base_dir.join(relative_path);
        fs::read(path).map_err(mx::ErrorKind::IOError)
    }

    pub fn load_string(&self, relative_path: impl AsRef<Path>) -> mx::Result<String> {
        let path = self.base_dir.join(relative_path);
        fs::read_to_string(path).map_err(mx::ErrorKind::IOError)
    }

    pub fn exists(&self, relative_path: impl AsRef<Path>) -> bool {
        self.base_dir.join(relative_path).exists()
    }

    pub fn remove(&self, relative_path: impl AsRef<Path>) -> mx::Result<()> {
        let path = self.base_dir.join(relative_path);
        fs::remove_file(path).map_err(mx::ErrorKind::IOError)
    }

    pub fn get_path(&self) -> &PathBuf {
        &self.base_dir
    }
}
