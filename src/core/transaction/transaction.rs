use std::{collections::HashMap, fs, path, process};

use super::build_queue::BuildQueue;
use super::file_lock::NixFile;
use crate::{
    CONFIG_NAME,
    core::{list::List as mxList, transaction::file_lock::NixFilePermission},
    mx,
};

pub(crate) const LOCK_SKIP_REBUILD_FILE: &str = "/tmp/mx-skip-rebuild.lock";

/// `nixos-rebuild` (or `nixos-install`) command to run after a successful commit.
///
/// In `debug` mode (without `--release`), all variants trigger `build-vm` to
/// avoid modifying the host system during development.
#[derive(Clone)]
pub enum BuildCommand {
    /// Rebuilds the system and switches immediately (`nixos-rebuild switch`).
    Switch,
    /// Prepares the next boot without rebooting (`nixos-rebuild boot`).
    Boot,
    /// Initial install on a new machine (`nixos-install`).
    /// The build command is empty in release; triggers `build-vm` in debug.
    Install,
}

pub enum UpdateInput {
    Keep,
    UpdateAll,
    UpdateSelected(Vec<String>),
}

pub enum TransactionPermission {
    ReadOnly,
    Writtable,
}

impl From<&TransactionPermission> for bool {
    fn from(p: &TransactionPermission) -> bool {
        matches!(p, TransactionPermission::Writtable)
    }
}

impl From<&TransactionPermission> for NixFilePermission {
    fn from(p: &TransactionPermission) -> NixFilePermission {
        match p {
            TransactionPermission::ReadOnly => NixFilePermission::ReadOnly,
            TransactionPermission::Writtable => NixFilePermission::Writtable,
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// LockFile – lightweight POSIX file lock
// ─────────────────────────────────────────────────────────────────────────────

/// File lock used to serialize NixOS builds.
///
/// The lock is acquired on creation via [`LockFile::try_lock`] and released
/// explicitly via [`LockFile::unlock`]. If `unlock` is not called, the lock is
/// released by the kernel when the process exits (but not when the Rust `File`
/// is dropped — prefer an explicit `unlock`).
struct LockFile {
    /// Handle to the locked file. `None` after an `unlock`.
    file: Option<fs::File>,
}

impl LockFile {
    /// Attempts to take a non-blocking exclusive lock.
    ///
    /// # Returns
    /// * `Ok(Some(lock))` – Lock acquired.
    /// * `Ok(None)`       – The file is already locked by another process.
    /// * `Err(_)`         – Unexpected I/O error.
    pub fn try_lock(path: &str) -> mx::Result<Option<Self>> {
        Ok(Some(LockFile {
            file: match fs::File::create(path) {
                Ok(f) => match f.try_lock() {
                    Ok(_) => Some(f),
                    Err(fs::TryLockError::WouldBlock) => return Ok(None),
                    Err(_) => return Err(mx::ErrorKind::FailToLock),
                },
                Err(e) => return Err(mx::ErrorKind::IOError(e)),
            },
        }))
    }

    /// Releases the lock and closes the handle. No-op if already unlocked.
    pub fn unlock(&mut self) {
        if self.file.is_some() {
            self.file.as_mut().unwrap().unlock().unwrap_or_default();
        }
        self.file = None;
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// BuildCommand – rebuild command selection
// ─────────────────────────────────────────────────────────────────────────────

impl BuildCommand {
    /// Returns the argument passed to `nixos-rebuild` for this command.
    ///
    /// In release mode:
    /// * `Switch`  → `"switch"`
    /// * `Boot`    → `"boot"`
    /// * `Install` → `""` (uses `nixos-install` directly, see [`Transaction::rebuild_config`])
    ///
    /// In debug mode: all variants return `"build-vm"` so as not to modify the
    /// host system.
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
            BuildCommand::Install => "build-vm",
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Transaction
// ─────────────────────────────────────────────────────────────────────────────

/// Atomic unit of work on a NixOS configuration Git repository.
///
/// A `Transaction` groups a set of [`NixFile`]s to edit, applies the changes in
/// memory, then either commits them (`commit`) — which triggers a Git commit and
/// a NixOS rebuild — or rolls them back (`rollback`) — which restores the
/// previous Git state and puts the files back in place.
///
/// # Lifecycle
/// ```text
/// Transaction::new(...)
///   └─ add_file(path)   // before begin
///   └─ begin()          // opens the Git repo, locks the files
///       └─ get_file(path) → &mut NixFile  // in-memory edits
///       └─ commit()     // writes to disk, Git commit, rebuild
///         or rollback() // undoes everything, restores previous state
/// ```
///
/// # Invariants
/// * `git_repo.is_some()` ⟺ active transaction (between `begin` and `commit`/`rollback`).
/// * `old_commit` holds the OID of the HEAD commit at `begin` time, allowing a
///   precise rollback even if files were created.
pub struct Transaction<'a> {
    /// Human-readable description of the transaction, used as the Git commit message.
    info: String,

    /// Map associating each relative path with its corresponding [`NixFile`].
    list_file: HashMap<String, NixFile>,

    /// Absolute path to the root of the NixOS configuration Git repository.
    git_repo_path: String,

    /// Handle to the Git repository, present only during an active transaction.
    git_repo: Option<git2::Repository>,

    /// Git identity used as author and committer.
    git_user: git2::Signature<'a>,

    /// Rebuild command to run after the commit.
    build_type: BuildCommand,

    /// OID of the HEAD commit captured at `begin`, used as the rollback target.
    /// Equals `Oid::zero()` if the repository was empty.
    old_commit: git2::Oid,

    /// OID of the stash commit created by [`begin`] if the repository contained
    /// uncommitted changes. `None` if no stash was needed.
    /// Restored automatically by [`commit`] and [`rollback`].
    stash_oid: Option<git2::Oid>,

    permission_transaction: TransactionPermission,
}

impl<'a> Transaction<'a> {
    /// Creates a new transaction without opening it.
    ///
    /// No Git or I/O operation is performed here.
    ///
    /// # Arguments
    /// * `config_dir`               – Path to the NixOS Git repository.
    /// * `transaction_description`  – Git commit message.
    /// * `build_type`               – Command to run after the commit.
    pub fn new(
        config_dir: &str,
        transaction_description: &str,
        build_type: BuildCommand,
        permission: TransactionPermission,
    ) -> mx::Result<Self> {
        Ok(Transaction {
            info: transaction_description.to_string(),
            list_file: HashMap::new(),
            git_repo: None,
            git_repo_path: config_dir.to_string(),
            git_user: git2::Signature::now("Modulix-OS", "modulix.os@ik-mail.com").unwrap(),
            build_type,
            old_commit: git2::Oid::ZERO_SHA1,
            stash_oid: None,
            permission_transaction: permission,
        })
    }

    /// Runs the NixOS rebuild in a subprocess and waits for it to finish.
    ///
    /// Depending on the `build_command` variant:
    /// * [`BuildCommand::Install`] → `nixos-install --root /mnt --no-root-password --flake …`
    /// * [`BuildCommand::Switch`] / [`BuildCommand::Boot`] → `nixos-rebuild <cmd> --flake …`
    ///
    /// Standard output is inherited (visible in the parent terminal); standard
    /// error is captured into `stderr` if provided.
    ///
    /// # Returns
    /// `Ok(true)` if the process exited successfully (code 0), `Ok(false)` otherwise.
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

    /// Checks whether `flake.lock` was modified (tracked or untracked) in the Git repo.
    ///
    /// Used before each commit to automatically include Nix lockfile updates in
    /// the Git commit.
    ///
    /// # Errors
    /// `mx::ErrorKind::TransactionNotBegin` if the transaction is not active.
    fn flake_lock_modified(&self) -> mx::Result<bool> {
        let repo = self
            .git_repo
            .as_ref()
            .ok_or(mx::ErrorKind::TransactionNotBegin)?;

        let statuses = repo.statuses(None).map_err(mx::ErrorKind::GitError)?;

        Ok(statuses.iter().any(|s| {
            s.path() == Ok("flake.lock")
                && s.status().intersects(
                    git2::Status::WT_MODIFIED
                        | git2::Status::WT_NEW
                        | git2::Status::INDEX_MODIFIED
                        | git2::Status::INDEX_NEW,
                )
        }))
    }

    /// Returns `true` if `flake.lock` physically exists in the repository directory.
    ///
    /// If the file is absent, a `nix flake update` will be run before the commit
    /// to generate the initial lockfile.
    fn flake_lock_exists(&self) -> bool {
        path::Path::new(&self.git_repo_path)
            .join("flake.lock")
            .exists()
    }

    /// Creates a Git commit with the current working tree.
    ///
    /// If `flake.lock` was modified, it is automatically added to the index
    /// before creating the commit.
    ///
    /// The commit is created without a parent if the repository is empty (first commit).
    ///
    /// # Arguments
    /// * `update_ref`  – Reference to update (e.g. `Some("HEAD")`).
    /// * `author`      – Author signature.
    /// * `committer`   – Committer signature.
    /// * `message`     – Commit message.
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

        // Include flake.lock if modified
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

        // Get the parent commit if it exists (None for the first commit)
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

    /// Determines whether a file was modified since commit `oid`.
    ///
    /// Used in [`commit_impl`] to include only the actually modified files in the
    /// Git commit, avoiding empty commits.
    ///
    /// If `oid` is zero (empty repository), the file is always considered new.
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

        // New file: necessarily different
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

    /// Adds a file to the Git index (equivalent to `git add <path>`).
    fn git_add(&self, path: &str) -> Result<(), mx::ErrorKind> {
        let repo = self.git_repo.as_ref().unwrap();
        let mut index = repo.index().map_err(mx::ErrorKind::GitError)?;
        index
            .add_path(path::Path::new(path))
            .map_err(mx::ErrorKind::GitError)?;
        index.write().map_err(mx::ErrorKind::GitError)?;
        Ok(())
    }

    /// Registers a Nix file to include in the transaction.
    ///
    /// Must be called **before** [`begin`]. Calling this method after `begin`
    /// returns `mx::ErrorKind::TransactionAlreadyBegin`.
    ///
    /// `configuration.nix` is added automatically by [`begin`]; there is no need
    /// to add it manually.
    ///
    /// # Arguments
    /// * `path` – Path relative to the repository root (e.g. `"/services/nginx.nix"`).
    pub fn add_file(&mut self, path: &str) -> mx::Result<()> {
        if self.git_repo.is_some() {
            return Err(mx::ErrorKind::TransactionAlreadyBegin);
        }
        self.list_file
            .insert(path.to_string(), NixFile::new(&self.git_repo_path, path));
        Ok(())
    }

    /// Reports whether a transaction is currently active.
    #[allow(dead_code)]
    pub fn as_begin(&self) -> bool {
        self.git_repo.is_some()
    }

    /// Returns a mutable reference to the [`NixFile`] associated with `path`.
    ///
    /// # Errors
    /// * `mx::ErrorKind::TransactionNotBegin` – `begin` has not been called yet.
    /// * `mx::ErrorKind::FileNotFound`        – `path` was not added via `add_file`.
    pub fn get_file_mut(&mut self, path: &str) -> mx::Result<&mut NixFile> {
        if self.git_repo.is_none() {
            return Err(mx::ErrorKind::TransactionNotBegin);
        }
        if let TransactionPermission::ReadOnly = self.permission_transaction {
            return Err(mx::ErrorKind::PermissionDenied);
        }
        self.list_file
            .get_mut(path)
            .ok_or(mx::ErrorKind::FileNotFound)
    }

    pub fn get_file(&mut self, path: &str) -> mx::Result<&NixFile> {
        if self.git_repo.is_none() {
            return Err(mx::ErrorKind::TransactionNotBegin);
        }
        self.list_file.get(path).ok_or(mx::ErrorKind::FileNotFound)
    }

    /// Opens the transaction: initializes the Git repository, stashes any
    /// uncommitted changes, locks and loads all registered files.
    ///
    /// Steps performed:
    /// 1. Automatically adds `configuration.nix` to the tracked files.
    /// 2. Opens the Git repository at `git_repo_path`.
    /// 3. If the repository contains uncommitted changes, they are stashed with
    ///    `INCLUDE_UNTRACKED` and restored automatically at the end of the transaction.
    /// 4. Calls [`NixFile::begin`] on each file; creates missing files and adds
    ///    them to the `imports` list of `configuration.nix`.
    /// 5. Captures the OID of the current HEAD commit for a possible rollback.
    ///
    /// # Errors
    /// * `mx::ErrorKind::GitError`                – Repository not found or Git error.
    /// * `mx::ErrorKind::TransactionAlreadyBegin` – `begin` already called.
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

            // If the repository contains uncommitted changes, stash them to work
            // on a clean tree and restore them afterwards.
            if !is_empty {
                let is_dirty = {
                    let mut opts = git2::StatusOptions::new();
                    opts.include_untracked(true).include_ignored(false);
                    let statuses = self
                        .git_repo
                        .as_ref()
                        .unwrap()
                        .statuses(Some(&mut opts))
                        .map_err(mx::ErrorKind::GitError)?;
                    !statuses.is_empty()
                }; // `statuses` is dropped here, releasing the immutable borrow

                if is_dirty {
                    let stash_oid = self
                        .git_repo
                        .as_mut()
                        .unwrap()
                        .stash_save(
                            &self.git_user,
                            "mx: auto-stash before transaction",
                            Some(git2::StashFlags::INCLUDE_UNTRACKED),
                        )
                        .map_err(mx::ErrorKind::GitError)?;
                    self.stash_oid = Some(stash_oid);
                }
            }

            for (path_file, file) in self.list_file.iter_mut() {
                match file.begin(NixFilePermission::from(&self.permission_transaction)) {
                    Ok(_) => (),
                    Err(mx::ErrorKind::FileNotFound)
                        if let TransactionPermission::Writtable = self.permission_transaction =>
                    {
                        // The file does not exist yet: create it and note that it
                        // must be declared in configuration.nix
                        file.create_file()?;
                        file.begin(NixFilePermission::Writtable)?;
                        new_file.push(path_file.clone());
                    }
                    Err(e) => return Err(e),
                }
            }

            // Capture the current commit for rollback
            self.old_commit = match self.git_repo.as_ref().unwrap().head() {
                Ok(head) => head.peel_to_commit().map_err(mx::ErrorKind::GitError)?.id(),
                Err(e)
                    if e.code() == git2::ErrorCode::UnbornBranch
                        || e.code() == git2::ErrorCode::NotFound =>
                {
                    git2::Oid::ZERO_SHA1
                }
                Err(e) => return Err(mx::ErrorKind::GitError(e)),
            };
        }
        if let TransactionPermission::Writtable = self.permission_transaction {
            // Add the new files to the imports list of configuration.nix
            let config_file = self.get_file_mut("configuration.nix")?;
            let import_file = mxList::new("imports", true);
            for path in new_file {
                import_file.add(config_file, &format!("./{}", &path))?;
            }
        }
        Ok(())
    }

    /// Restores the stash created by [`begin`], if any.
    ///
    /// Called at the end of [`commit_impl`] and [`rollback`] to put back the
    /// changes that were present before the transaction was opened.
    ///
    /// If `stash_pop` fails (conflict), the stash entry is dropped instead; either
    /// way `stash_oid` is reset to avoid a double attempt.
    fn stash_restore(&mut self) -> mx::Result<()> {
        if self.stash_oid.take().is_some() {
            match self.git_repo.as_mut().unwrap().stash_pop(0, None) {
                Ok(_) => (),
                Err(_) => {
                    self.git_repo
                        .as_mut()
                        .unwrap()
                        .stash_drop(0)
                        .map_err(mx::ErrorKind::GitError)?;
                }
            }
        }
        Ok(())
    }

    /// Internal commit implementation, split out so the [`commit`] wrapper can
    /// trigger an automatic rollback on failure.
    ///
    /// Steps:
    /// 1. Commit each [`NixFile`] to disk.
    /// 2. Detect the actually modified files (selective `git add`).
    /// 3. If at least one file changed:
    ///    a. Generate `flake.lock` if absent (`nix flake update`).
    ///    b. Create the Git commit.
    ///    c. Enter the FIFO build queue and, once at the head, run `nixos-rebuild`.
    /// 4. Close all [`NixFile`]s and release the Git repository.
    fn commit_impl(&mut self, update_input: UpdateInput) -> mx::Result<()> {
        if self.git_repo.is_none() {
            return Err(mx::ErrorKind::TransactionNotBegin);
        }

        let mut need_modif = false;
        if let TransactionPermission::Writtable = self.permission_transaction {
            for (_, nix_file) in self.list_file.iter_mut() {
                nix_file.commit()?;
            }
            for (path, _) in self.list_file.iter() {
                if Self::has_diff_with_commit(
                    self.git_repo.as_ref().unwrap(),
                    self.old_commit,
                    path,
                )? {
                    need_modif = true;
                    self.git_add(path)?;
                }
            }
        }

        if need_modif {
            // Generate flake.lock if it does not exist yet
            if !self.flake_lock_exists() {
                process::Command::new("nix")
                    .args(["flake", "update"])
                    .current_dir(&self.git_repo_path)
                    .output()
                    .map_err(mx::ErrorKind::IOError)?;
            } else {
                let mut args = vec!["flake".to_string(), "update".to_string()];
                let command = match update_input {
                    UpdateInput::UpdateAll => args,
                    UpdateInput::UpdateSelected(s) => {
                        args.extend(s);
                        args
                    }
                    UpdateInput::Keep => vec![],
                };
                if !command.is_empty() {
                    process::Command::new("nix")
                        .args(command)
                        .current_dir(&self.git_repo_path)
                        .output()
                        .map_err(mx::ErrorKind::IOError)?;
                }
            }
            self.git_commit(Some("HEAD"), &self.git_user, &self.git_user, &self.info)?;

            // Test/maintenance hook: if the sentinel is already held, skip the
            // rebuild (tests hold it to avoid nixos-rebuild and to serialize
            // access to the shared fixture repo).
            let skip = LockFile::try_lock(LOCK_SKIP_REBUILD_FILE)?;
            if let Some(mut sentinel) = skip {
                sentinel.unlock(); // sentinel free → real run

                // Strict FIFO queue: a single operation is rebuilt at a time, in
                // arrival order. Blocks until at the head of the queue.
                let ticket = BuildQueue::enqueue()?;
                ticket.wait_turn()?;

                let mut stderr = String::new();
                let success = Self::rebuild_config(
                    &self.git_repo_path,
                    CONFIG_NAME,
                    self.build_type.clone(),
                    Some(&mut stderr),
                )?;
                // `ticket` is dropped at the end of the block (or on early-return) → dequeue.
                if !success {
                    return Err(mx::ErrorKind::BuildError(stderr));
                }
            }
        }

        for (_, nix_file) in self.list_file.iter_mut() {
            nix_file.close()?;
        }
        // Restore the changes stashed before the transaction
        self.stash_restore()?;
        self.git_repo = None;
        Ok(())
    }
    /// Persists the changes, creates a Git commit and triggers the NixOS rebuild.
    ///
    /// On internal failure, an automatic [`rollback`] is attempted before
    /// propagating the error.
    pub fn commit(&mut self, update_input: UpdateInput) -> mx::Result<()> {
        self.commit_impl(update_input).map_err(|e| {
            let _ = self.rollback();
            e
        })
    }

    /// Cancels the transaction and restores the previous Git repository state.
    ///
    /// Steps:
    /// 1. If the repository was empty at `begin` (`old_commit` zero): close the
    ///    files and return without touching Git.
    /// 2. Otherwise: repoint the current branch to `old_commit` and perform a
    ///    `checkout --force` to restore the working tree.
    /// 3. Remove the files created during the transaction; re-apply the immutable
    ///    flag on the restored pre-existing files.
    ///
    /// # Errors
    /// `mx::ErrorKind::TransactionNotBegin` if no transaction is active.
    pub fn rollback(&mut self) -> mx::Result<()> {
        if self.git_repo.is_none() {
            return Err(mx::ErrorKind::TransactionNotBegin);
        }

        {
            // Special case: empty repository, no commit to restore
            if self.old_commit.is_zero() {
                for (_, nix_file) in self.list_file.iter_mut() {
                    let _ = nix_file.close();
                }
                self.git_repo = None;
                return Ok(());
            }

            let repo = self.git_repo.as_ref().unwrap();
            let head = repo.head().map_err(mx::ErrorKind::GitError)?;

            let refname = head.name().map_err(|_| {
                mx::ErrorKind::GitError(git2::Error::from_str("HEAD is not a symbolic ref"))
            })?;

            // Repoint the HEAD reference to the old commit
            repo.find_reference(refname)
                .map_err(mx::ErrorKind::GitError)?
                .set_target(self.old_commit, "reset to previous commit")
                .map_err(mx::ErrorKind::GitError)?;

            repo.set_head(refname).map_err(mx::ErrorKind::GitError)?;

            // Make the files mutable so checkout can overwrite them
            for (_, nix_file) in self.list_file.iter_mut() {
                NixFile::make_mutable(nix_file.get_file_path()).ok();
            }

            // Force the working tree restoration
            let mut checkout = git2::build::CheckoutBuilder::new();
            checkout.force();
            repo.checkout_head(Some(&mut checkout))
                .map_err(mx::ErrorKind::GitError)?;

            // Post-checkout cleanup:
            // - Files created during the transaction → removed
            // - Pre-existing files → immutable flag re-applied
            for (_, nix_file) in self.list_file.iter_mut() {
                if nix_file.was_created() {
                    NixFile::make_mutable(nix_file.get_file_path()).ok();
                    std::fs::remove_file(nix_file.get_file_path()).ok();
                } else if path::Path::new(nix_file.get_file_path()).exists() {
                    NixFile::make_immutable(nix_file.get_file_path()).ok();
                }
            }

            // Release the locks and reset the state of each NixFile.
            // Without this close(), the file lock would stay active after rollback,
            // blocking any later begin() on the same file indefinitely.
            for (_, nix_file) in self.list_file.iter_mut() {
                let _ = nix_file.close();
            }
        }
        // Restore the changes stashed before the transaction
        self.stash_restore()?;
        self.git_repo = None;
        Ok(())
    }
}

#[cfg(test)]
#[path = "transaction_tests.rs"]
mod tests;
