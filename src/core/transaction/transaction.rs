use std::{collections::HashMap, fs, path, process};

use super::file_lock::NixFile;
use crate::{CONFIG_NAME, core::list::List as mxList, mx};

const LOCK_BUILD_FILE: &str = "/tmp/mx-build.lock";
const LOCK_QUEUE_BUILD_FILE: &str = "/tmp/mx-queue-build.lock";

#[derive(Clone)]
pub enum BuildCommand {
    Switch,
    Boot,
    Install,
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
                        return Err(mx::ErrorKind::FailToLock);
                    }
                },
                Err(e) => return Err(mx::ErrorKind::IOError(e)),
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
                        return Err(mx::ErrorKind::FailToLock);
                    }
                },
                Err(e) => return Err(mx::ErrorKind::IOError(e)),
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
            BuildCommand::Boot => "boot",
            BuildCommand::Install => "",
        }
    }
    #[cfg(debug_assertions)]
    pub fn as_str(&self) -> &'static str {
        match self {
            BuildCommand::Switch => "build-vm",
            BuildCommand::Boot => "build-vm",
            BuildCommand::Install => "",
        }
    }
}

pub struct Transaction<'a> {
    info: String,
    list_file: HashMap<String, NixFile>,
    git_repo_path: String,
    git_repo: Option<git2::Repository>,
    git_user: git2::Signature<'a>,
    build_type: BuildCommand,
    old_commit: git2::Oid,
}

impl<'a> Transaction<'a> {
    pub fn new(
        config_dir: &str,
        transaction_description: &str,
        build_type: BuildCommand,
    ) -> mx::Result<Self> {
        Ok(Transaction {
            info: transaction_description.to_string(),
            list_file: HashMap::new(),
            git_repo: None,
            git_repo_path: config_dir.to_string(),
            git_user: git2::Signature::now("Modulix-OS", "modulix.os@ik-mail.com").unwrap(),
            build_type: build_type,
            old_commit: git2::Oid::zero(),
        })
    }

    fn rebuild_config(
        path_config: &str,
        config_name: &str,
        build_command: BuildCommand,
        stderr: Option<&mut String>,
    ) -> mx::Result<bool> {
        let mut child = match build_command {
            BuildCommand::Install => process::Command::new("nixos-install")
                .arg("--root")
                .arg("/mnt")
                .arg("--no-root-password")
                .arg("--flake")
                .arg(format!("{}#{}", path_config, config_name))
                .stdout(process::Stdio::inherit())
                .stderr(process::Stdio::piped())
                .spawn()
                .map_err(mx::ErrorKind::IOError)?,
            BuildCommand::Switch | BuildCommand::Boot => process::Command::new("nixos-rebuild")
                .arg(build_command.as_str())
                .arg("--flake")
                .arg(format!("{}#{}", path_config, config_name))
                .stdout(process::Stdio::inherit())
                .stderr(process::Stdio::piped())
                .spawn()
                .map_err(mx::ErrorKind::IOError)?,
        };

        let stderr_output = {
            let mut s = String::new();
            if let Some(mut err) = child.stderr.take() {
                use std::io::Read;
                err.read_to_string(&mut s).map_err(mx::ErrorKind::IOError)?;
            }
            s
        };
        let status = child.wait().map_err(mx::ErrorKind::IOError)?;
        if let Some(s) = stderr {
            *s = stderr_output;
        }
        Ok(status.success())
    }

    fn flake_lock_modified(&self) -> mx::Result<bool> {
        let repo = self
            .git_repo
            .as_ref()
            .ok_or(mx::ErrorKind::TransactionNotBegin)?;

        let statuses = repo.statuses(None).map_err(mx::ErrorKind::GitError)?;

        Ok(statuses.iter().any(|s| {
            s.path() == Some("flake.lock")
                && s.status().intersects(
                    git2::Status::WT_MODIFIED
                        | git2::Status::WT_NEW
                        | git2::Status::INDEX_MODIFIED
                        | git2::Status::INDEX_NEW,
                )
        }))
    }

    fn flake_lock_exists(&self) -> bool {
        path::Path::new(&self.git_repo_path)
            .join("flake.lock")
            .exists()
    }

    fn git_commit(
        &self,
        update_ref: Option<&str>,
        author: &git2::Signature<'_>,
        committer: &git2::Signature<'_>,
        message: &str,
    ) -> mx::Result<()> {
        let mut index = self
            .git_repo
            .as_ref()
            .unwrap()
            .index()
            .map_err(mx::ErrorKind::GitError)?;

        if self.flake_lock_modified()? {
            index
                .add_path(std::path::Path::new("flake.lock"))
                .map_err(mx::ErrorKind::GitError)?;
            index.write().map_err(mx::ErrorKind::GitError)?;
        }

        let tree_oid = index.write_tree().map_err(mx::ErrorKind::GitError)?;
        let tree = self
            .git_repo
            .as_ref()
            .unwrap()
            .find_tree(tree_oid)
            .map_err(mx::ErrorKind::GitError)?;
        let parent = self
            .git_repo
            .as_ref()
            .unwrap()
            .head()
            .and_then(|h| h.peel_to_commit())
            .ok();

        let parents: Vec<&git2::Commit> = parent.iter().collect();

        self.git_repo
            .as_ref()
            .unwrap()
            .commit(update_ref, author, committer, message, &tree, &parents)
            .map_err(mx::ErrorKind::GitError)?;
        Ok(())
    }

    fn has_diff_with_commit(
        repo: &git2::Repository,
        oid: git2::Oid,
        file_path: &str,
    ) -> mx::Result<bool> {
        if oid.is_zero() {
            return Ok(true);
        }
        let commit = repo.find_commit(oid).unwrap();
        let commit_tree = commit.tree().unwrap();

        let status = repo
            .status_file(path::Path::new(file_path))
            .map_err(mx::ErrorKind::GitError)?;
        if status.contains(git2::Status::WT_NEW) || status.contains(git2::Status::INDEX_NEW) {
            return Ok(true);
        }

        let mut diff_opts = git2::DiffOptions::new();
        diff_opts.pathspec(path::Path::new(file_path));
        let diff = repo
            .diff_tree_to_workdir_with_index(Some(&commit_tree), Some(&mut diff_opts))
            .unwrap();
        Ok(diff.stats().unwrap().files_changed() > 0)
    }

    fn git_add(&self, path: &str) -> Result<(), mx::ErrorKind> {
        let repo = self.git_repo.as_ref().unwrap();
        let mut index = repo.index().map_err(mx::ErrorKind::GitError)?;
        index
            .add_path(path::Path::new(path))
            .map_err(mx::ErrorKind::GitError)?;
        index.write().map_err(mx::ErrorKind::GitError)?;
        Ok(())
    }

    pub fn add_file(&mut self, path: &str) -> mx::Result<()> {
        if self.git_repo.is_some() {
            return Err(mx::ErrorKind::TransactionAlreadyBegin);
        }
        self.list_file
            .insert(path.to_string(), NixFile::new(&self.git_repo_path, path));
        Ok(())
    }

    #[allow(dead_code)]
    pub fn as_begin(&self) -> bool {
        return self.git_repo.is_some();
    }

    pub fn get_file(&mut self, path: &str) -> mx::Result<&mut NixFile> {
        if self.git_repo.is_none() {
            return Err(mx::ErrorKind::TransactionNotBegin);
        }
        return self
            .list_file
            .get_mut(path)
            .ok_or(mx::ErrorKind::FileNotFound);
    }

    pub fn begin(&mut self) -> mx::Result<()> {
        self.add_file("configuration.nix")?;
        let mut new_file: Vec<String> = vec![];
        {
            self.git_repo =
                Some(git2::Repository::open(&self.git_repo_path).map_err(mx::ErrorKind::GitError)?);

            let is_empty = self
                .git_repo
                .as_ref()
                .unwrap()
                .is_empty()
                .map_err(mx::ErrorKind::GitError)?;

            if !is_empty {
                let mut opts = git2::StatusOptions::new();
                opts.include_untracked(true).include_ignored(false);

                let statuses = self
                    .git_repo
                    .as_ref()
                    .unwrap()
                    .statuses(Some(&mut opts))
                    .map_err(mx::ErrorKind::GitError)?;

                if !statuses.is_empty() {
                    return Err(mx::ErrorKind::GitNotCommitted);
                }
            }

            for (path_file, file) in self.list_file.iter_mut() {
                match file.begin() {
                    Ok(_) => (),
                    Err(mx::ErrorKind::FileNotFound) => {
                        file.create_file()?;
                        file.begin()?;
                        new_file.push(path_file.clone());
                    }
                    Err(e) => return Err(e),
                }
            }

            self.old_commit = match self.git_repo.as_ref().unwrap().head() {
                Ok(head) => head.peel_to_commit().map_err(mx::ErrorKind::GitError)?.id(),
                Err(e)
                    if e.code() == git2::ErrorCode::UnbornBranch
                        || e.code() == git2::ErrorCode::NotFound =>
                {
                    git2::Oid::zero()
                }
                Err(e) => return Err(mx::ErrorKind::GitError(e)),
            };
        }
        {
            let config_file = self.get_file("configuration.nix")?;
            let import_file = mxList::new("imports", true);
            for path in new_file {
                import_file.add(config_file, &format!("./{}", &path))?;
            }
        }
        Ok(())
    }

    fn commit_impl(&mut self) -> mx::Result<()> {
        if self.git_repo.is_none() {
            return Err(mx::ErrorKind::TransactionNotBegin);
        }
        for (_, nix_file) in self.list_file.iter_mut() {
            nix_file.commit()?;
        }
        let mut need_modif = false;
        for (path, _) in self.list_file.iter() {
            if Self::has_diff_with_commit(&self.git_repo.as_mut().unwrap(), self.old_commit, path)?
            {
                need_modif = true;
                self.git_add(&path)?;
            }
        }
        if need_modif {
            if !self.flake_lock_exists() {
                process::Command::new("nix")
                    .args(["flake", "update"])
                    .current_dir(&self.git_repo_path)
                    .output()
                    .map_err(mx::ErrorKind::IOError)?;
            }
            self.git_commit(Some("HEAD"), &self.git_user, &self.git_user, &self.info)?;
            let mut queue = LockFile::try_lock(LOCK_QUEUE_BUILD_FILE)?;
            if queue.is_some() {
                let mut lock_build = LockFile::lock(LOCK_BUILD_FILE)?;
                queue.as_mut().unwrap().unlock();
                let mut stderr = String::new();
                let success = Self::rebuild_config(
                    &self.git_repo_path,
                    CONFIG_NAME,
                    self.build_type.clone(),
                    Some(&mut stderr),
                )?;
                lock_build.unlock();
                if !success {
                    return Err(mx::ErrorKind::BuildError(stderr));
                }
            }
        }
        for (_, nix_file) in self.list_file.iter_mut() {
            nix_file.close()?;
        }
        self.git_repo = None;
        Ok(())
    }

    pub fn commit(&mut self) -> mx::Result<()> {
        self.commit_impl().map_err(|e| {
            let _ = self.rollback();
            e
        })
    }

    pub fn rollback(&mut self) -> mx::Result<()> {
        if self.git_repo.is_none() {
            return Err(mx::ErrorKind::TransactionNotBegin);
        }

        {
            if self.old_commit.is_zero() {
                for (_, nix_file) in self.list_file.iter_mut() {
                    let _ = nix_file.close();
                }
                self.git_repo = None;
                return Ok(());
            }

            let repo = self.git_repo.as_ref().unwrap();
            let head = repo.head().map_err(mx::ErrorKind::GitError)?;

            let refname = head
                .name()
                .ok_or(mx::ErrorKind::GitError(git2::Error::from_str(
                    "HEAD is not a symbolic ref",
                )))?;

            repo.find_reference(refname)
                .map_err(mx::ErrorKind::GitError)?
                .set_target(self.old_commit, "reset to previous commit")
                .map_err(mx::ErrorKind::GitError)?;

            repo.set_head(refname).map_err(mx::ErrorKind::GitError)?;

            for (_, nix_file) in self.list_file.iter_mut() {
                NixFile::make_mutable(nix_file.get_file_path()).ok();
            }

            let mut checkout = git2::build::CheckoutBuilder::new();
            checkout.force();
            repo.checkout_head(Some(&mut checkout))
                .map_err(mx::ErrorKind::GitError)?;

            for (_, nix_file) in self.list_file.iter_mut() {
                if nix_file.was_created() {
                    NixFile::make_mutable(nix_file.get_file_path()).ok();
                    std::fs::remove_file(nix_file.get_file_path()).ok();
                } else if path::Path::new(nix_file.get_file_path()).exists() {
                    NixFile::make_immutable(nix_file.get_file_path()).ok();
                }
            }
        }
        self.git_repo = None;
        Ok(())
    }
}
