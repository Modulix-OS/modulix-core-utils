use std::{collections::HashMap, fs, path, process, thread, time};

use crate::{mx, transaction::file_lock::NixFile};

const LOCK_BUILD_FILE: &str = "/tmp/mx-build.lock";
const LOCK_QUEUE_BUILD_FILE: &str = "/tmp/mx-queue-build.lock";
const LOCK_GIT: &str = "/tmp/mx-git.lock";
const CONFIG_DIR: &str = "/etc/nixos";
const CONFIG_NAME: &str = "default";

#[derive(Clone)]
pub enum BuildCommand {
    Switch,
    Build,
}

struct LockFile {
    file: Option<fs::File>,
}

impl LockFile {
    pub fn lock(path: &str) -> mx::Result<Self> {
        Ok(LockFile {
            file: match fs::File::create(path) {
                Ok(f) => match f.lock() {
                    Ok(_) => Some(f),
                    Err(_) => {
                        return Err(mx::ErrorType::FailToLock);
                    }
                },
                Err(e) => return Err(mx::ErrorType::IOError(e)),
            },
        })
    }

    // Ok(None) if lock fail
    pub fn try_lock(path: &str) -> mx::Result<Option<Self>> {
        Ok(Some(LockFile {
            file: match fs::File::create(path) {
                Ok(f) => match f.try_lock() {
                    Ok(_) => Some(f),
                    Err(fs::TryLockError::WouldBlock) => return Ok(None),
                    Err(_) => {
                        return Err(mx::ErrorType::FailToLock);
                    }
                },
                Err(e) => return Err(mx::ErrorType::IOError(e)),
            },
        }))
    }

    pub fn unlock(&mut self) {
        if self.file.is_some() {
            self.file.as_mut().unwrap().unlock().unwrap_or_default();
        }
        self.file = None;
    }
}

impl BuildCommand {
    #[cfg(not(debug_assertions))]
    pub fn as_str(&self) -> &'static str {
        match self {
            BuildCommand::Switch => "switch",
            BuildCommand::Build => "build",
        }
    }
    #[cfg(debug_assertions)]
    pub fn as_str(&self) -> &'static str {
        match self {
            BuildCommand::Switch => "build-vm",
            BuildCommand::Build => "build-vm",
        }
    }
}

pub struct Transaction<'a> {
    info: String,
    list_file: HashMap<String, &'a mut NixFile>,
    git_repo: Option<git2::Repository>,
    git_user: git2::Signature<'a>,
    build_type: BuildCommand,
}

impl<'a> Transaction<'a> {
    pub fn new(transaction_description: &str, build_type: BuildCommand) -> mx::Result<Self> {
        Ok(Transaction {
            info: transaction_description.to_string(),
            list_file: HashMap::new(),
            git_repo: None,
            git_user: git2::Signature::now("Modulix-OS", "modulix.os@ik-mail.com").unwrap(),
            build_type: build_type,
        })
    }

    fn rebuild_config(
        path_config: &str,
        config_name: &str,
        build_command: BuildCommand,
    ) -> mx::Result<bool> {
        let status = match process::Command::new("nixos-rebuild")
            .arg(build_command.as_str())
            .arg("--flake")
            .arg(format!("{}#{}", path_config, config_name))
            .spawn()
        {
            Ok(mut child) => match child.wait() {
                Ok(status) => status,
                Err(e) => return Err(mx::ErrorType::IOError(e)),
            },
            Err(e) => return Err(mx::ErrorType::IOError(e)),
        };
        Ok(status.success())
    }

    fn git_commit(
        &self,
        update_ref: Option<&str>,
        author: &git2::Signature<'_>,
        committer: &git2::Signature<'_>,
        message: &str,
    ) -> mx::Result<()> {
        let mut index = match self.git_repo.as_ref().unwrap().index() {
            Ok(ind) => ind,
            Err(e) => return Err(mx::ErrorType::GitError(e)),
        };
        let tree_oid = match index.write_tree() {
            Ok(ind) => ind,
            Err(e) => return Err(mx::ErrorType::GitError(e)),
        };
        let tree = match self.git_repo.as_ref().unwrap().find_tree(tree_oid) {
            Ok(ind) => ind,
            Err(e) => return Err(mx::ErrorType::GitError(e)),
        };
        let parent = self
            .git_repo
            .as_ref()
            .unwrap()
            .head()
            .and_then(|h| h.peel_to_commit())
            .ok();
        let parents: Vec<&git2::Commit> = parent.iter().collect();
        match self
            .git_repo
            .as_ref()
            .unwrap()
            .commit(update_ref, author, committer, message, &tree, &parents)
        {
            Ok(_) => (),
            Err(e) => return Err(mx::ErrorType::GitError(e)),
        };
        Ok(())
    }

    fn git_add(&self, path: &str) -> mx::Result<()> {
        let mut index = match self.git_repo.as_ref().unwrap().index() {
            Ok(index) => index,
            Err(e) => return Err(mx::ErrorType::GitError(e)),
        };
        match index.add_path(path::Path::new(path)) {
            Ok(_) => (),
            Err(e) => return Err(mx::ErrorType::GitError(e)),
        }
        match index.write() {
            Ok(_) => (),
            Err(e) => return Err(mx::ErrorType::GitError(e)),
        };
        Ok(())
    }

    fn wait_until_clean(&self, timeout: time::Duration) -> bool {
        let start = std::time::Instant::now();
        loop {
            let mut opts = git2::StatusOptions::new();
            opts.include_untracked(false);

            let statuses = self
                .git_repo
                .as_ref()
                .unwrap()
                .statuses(Some(&mut opts))
                .unwrap();
            if statuses.is_empty() {
                return true;
            }
            if start.elapsed() >= timeout {
                return false;
            }

            thread::sleep(time::Duration::from_millis(500));
        }
    }

    pub(super) fn add_file(&mut self, nix_file: &'a mut NixFile) {
        self.list_file
            .insert(nix_file.get_file_path().to_string(), nix_file);
    }

    pub fn as_begin(&self) -> bool {
        return self.git_repo.is_some();
    }

    pub fn begin(&mut self) -> mx::Result<()> {
        // self.git_lock = Some(LockFile::lock(LOCK_GIT)?);

        self.git_repo = match git2::Repository::open(CONFIG_DIR) {
            Ok(repo) => Some(repo),
            Err(e) => return Err(mx::ErrorType::GitError(e)),
        };

        let mut opts = git2::StatusOptions::new();
        opts.include_untracked(true).include_ignored(false);

        let statuses = match self.git_repo.as_ref().unwrap().statuses(Some(&mut opts)) {
            Ok(s) => s,
            Err(e) => return Err(mx::ErrorType::GitError(e)),
        };

        if !statuses.is_empty() {
            return Err(mx::ErrorType::GitNotCommitted);
        }

        Ok(())
    }

    pub fn commit(&mut self) -> mx::Result<()> {
        if self.git_repo.is_none() {
            return Err(mx::ErrorType::TransactionNotBegin);
        }
        for (_, nix_file) in self.list_file.iter_mut() {
            nix_file.commit()?;
        }
        for (path, _) in self.list_file.iter() {
            self.git_add(&path)?;
        }
        if !self.wait_until_clean(time::Duration::from_mins(2)) {
            return Err(mx::ErrorType::InvalidFile);
        }
        let mut queue = LockFile::try_lock(LOCK_QUEUE_BUILD_FILE)?;
        if queue.is_some() {
            let mut lock_build = LockFile::lock(LOCK_BUILD_FILE)?;
            queue.as_mut().unwrap().unlock();
            let success = Self::rebuild_config(CONFIG_DIR, CONFIG_NAME, self.build_type.clone())?;
            lock_build.unlock();
            if !success {
                self.rollback()?;
                return Err(mx::ErrorType::InvalidFile);
            }
            self.git_commit(None, &self.git_user, &self.git_user, &self.info)?;
        }

        self.git_repo = None;
        Ok(())
    }

    pub fn rollback(&mut self) -> mx::Result<()> {
        if self.git_repo.is_none() {
            return Err(mx::ErrorType::TransactionNotBegin);
        }
        for (_, nix_file) in self.list_file.iter_mut() {
            nix_file.rollback()?;
        }
        self.git_repo = None;
        Ok(())
    }
}
