//! [`Transaction`]: a set of configuration files plus the rebuild that applies
//! them, committed to git or rolled back as one unit.

use std::{collections::HashMap, fs, io, path, process};

use super::build_queue::BuildQueue;
use super::file_lock::NixFile;
use crate::error::io_error_at;
use crate::{
    CONFIG_NAME,
    core::{list::List as mxList, transaction::file_lock::NixFilePermission},
    mx,
};

/// Sentinel whose presence-and-lockability tells a commit to skip the rebuild.
///
/// A test holds the lock on this file for as long as it wants commits to stay
/// purely textual; a commit that finds the lock taken edits and commits, but
/// runs no `nixos-rebuild`.
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
    /// Builds a VM image (`nixos-rebuild build-vm`), never touches the running
    /// system. Runtime-selected (not a debug-only gate) so a release binary
    /// (e.g. `mx-init --debug`) can also seed a disposable test VM.
    BuildVm,
}

/// How the commit refreshes `flake.lock`.
///
/// # Variants
/// * `Keep` - leave every input pinned where it is.
/// * `UpdateAll` - `nix flake update`, refreshing every input.
/// * `UpdateSelected` - refresh only the named inputs, which is what an
///   operation touching one input uses.
pub enum UpdateInput {
    Keep,
    UpdateAll,
    UpdateSelected(Vec<String>),
}

/// What a transaction is allowed to do with the configuration.
///
/// # Variants
/// * `ReadOnly` - files can be read under the transaction's locks, but not
///   edited, and there is nothing to commit.
/// * `Writtable` - files can be edited, committed and rebuilt.
pub enum TransactionPermission {
    ReadOnly,
    Writtable,
}

impl From<&TransactionPermission> for bool {
    /// Reduces a permission to its writability bit.
    ///
    /// # Parameters
    /// * `p` - the permission to convert.
    ///
    /// # Returns
    /// `true` for `Writtable`, `false` for `ReadOnly`.
    fn from(p: &TransactionPermission) -> bool {
        matches!(p, TransactionPermission::Writtable)
    }
}

impl From<&TransactionPermission> for NixFilePermission {
    /// Propagates a transaction's permission to the files it opens.
    ///
    /// # Parameters
    /// * `p` - the transaction's permission.
    ///
    /// # Returns
    /// The matching [`NixFilePermission`], so no file can be more permissive
    /// than its transaction.
    fn from(p: &TransactionPermission) -> NixFilePermission {
        match p {
            TransactionPermission::ReadOnly => NixFilePermission::ReadOnly,
            TransactionPermission::Writtable => NixFilePermission::Writtable,
        }
    }
}

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
    /// The file is opened read-only when it already exists, and only created
    /// when missing: `flock` does not need a writable descriptor, and these
    /// sentinels live in `/tmp`, where they are routinely left behind by
    /// another user (a root run, then a user run, or the reverse) — a
    /// create-always open would then fail with `EACCES` for no good reason.
    ///
    /// # Arguments
    /// * `path` – Sentinel file to lock; it is created if missing.
    ///
    /// # Returns
    /// * `Ok(Some(lock))` – Lock acquired.
    /// * `Ok(None)`       – The file is already locked by another process.
    /// * `Err(_)`         – Unexpected I/O error.
    ///
    /// # Post-conditions
    /// Never blocks. The lock lives until [`LockFile::unlock`] or, failing
    /// that, until the process exits.
    pub fn try_lock(path: &str) -> mx::Result<Option<Self>> {
        let opened = match fs::OpenOptions::new().read(true).open(path) {
            Ok(f) => Ok(f),
            Err(e) if e.kind() == io::ErrorKind::NotFound => fs::File::create(path),
            Err(e) => Err(e),
        };
        Ok(Some(LockFile {
            file: match opened {
                Ok(f) => match f.try_lock() {
                    Ok(_) => Some(f),
                    Err(fs::TryLockError::WouldBlock) => return Ok(None),
                    Err(_) => return Err(mx::ErrorKind::FailToLock),
                },
                Err(e) => return Err(io_error_at(path, e)),
            },
        }))
    }

    /// Releases the lock and closes the handle. No-op if already unlocked.
    ///
    /// # Post-conditions
    /// The sentinel file is left on disk, only unlocked. A failure to unlock is
    /// swallowed: the kernel releases it at process exit anyway.
    pub fn unlock(&mut self) {
        if self.file.is_some() {
            self.file.as_mut().unwrap().unlock().unwrap_or_default();
        }
        self.file = None;
    }
}

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
    ///
    /// # Returns
    /// The subcommand to hand `nixos-rebuild`; the empty string for `Install`,
    /// which uses `nixos-install` instead and ignores this value.
    #[cfg(not(debug_assertions))]
    pub fn as_str(&self) -> &'static str {
        match self {
            BuildCommand::Switch => "switch",
            BuildCommand::Boot => "boot",
            BuildCommand::Install => "",
            BuildCommand::BuildVm => "build-vm",
        }
    }

    /// Debug-build counterpart of the release `as_str`.
    ///
    /// # Returns
    /// Always `"build-vm"`, so a development run builds a VM image instead of
    /// touching the host system, whatever the variant asked for.
    #[cfg(debug_assertions)]
    pub fn as_str(&self) -> &'static str {
        match self {
            BuildCommand::Switch => "build-vm",
            BuildCommand::Boot => "build-vm",
            BuildCommand::Install => "build-vm",
            BuildCommand::BuildVm => "build-vm",
        }
    }
}

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

    /// Whether this transaction may edit and commit, or only read.
    permission_transaction: TransactionPermission,

    /// When `true`, [`commit_impl`] runs `run_flake_update` even if no file in
    /// `list_file` changed, so an operation that edits nothing (a system
    /// update) can still refresh `flake.lock`. The commit and the rebuild
    /// still only happen if that refresh actually moved `flake.lock`.
    force_commit: bool,

    /// `--cores` passed to `nixos-rebuild`/`nixos-install`, capping how many
    /// CPU cores a single derivation build may use. `None` leaves the Nix
    /// default (`nix.conf`'s `cores`, itself defaulting to all of them).
    rebuild_cores: Option<u32>,
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
    /// * `permission`               – Whether the files may be edited or only read.
    ///
    /// # Returns
    /// A transaction that is not open yet: [`Transaction::add_file`] then
    /// [`Transaction::begin`] are what touch the repository.
    ///
    /// # Post-conditions
    /// The author and committer identity of the future commit is fixed to
    /// `Modulix-OS <modulix.os@ik-mail.com>`, not the caller's git identity.
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
            force_commit: false,
            rebuild_cores: None,
        })
    }

    /// Forces [`commit_impl`] to run `run_flake_update` even when no tracked
    /// file changed.
    ///
    /// # Arguments
    /// * `force` - when `true`, an update-only transaction (no file edit)
    ///   still refreshes `flake.lock`; the commit and rebuild still only
    ///   happen if that refresh actually moved the lockfile.
    pub fn set_force_commit(&mut self, force: bool) {
        self.force_commit = force;
    }

    /// Caps the number of CPU cores the rebuild's `nix` build may use.
    ///
    /// # Arguments
    /// * `cores` - forwarded as `nixos-rebuild --cores <cores>` /
    ///   `nixos-install --cores <cores>`; `None` leaves the Nix default.
    pub fn set_cores(&mut self, cores: Option<u32>) {
        self.rebuild_cores = cores;
    }

    /// Builds the (unspawned) `nixos-install`/`nixos-rebuild` command for
    /// [`rebuild_config`], split out so the argument list can be asserted on
    /// in tests without spawning a real process.
    ///
    /// # Arguments
    /// * `path_config`, `config_name`, `build_command` – as in
    ///   [`rebuild_config`].
    /// * `cores` – when `Some`, appends `--cores <cores>`.
    ///
    /// # Returns
    /// The command, with its arguments set and stdio left at the
    /// `process::Command` default (the caller wires stdio and spawns it).
    fn build_rebuild_command(
        path_config: &str,
        config_name: &str,
        build_command: &BuildCommand,
        cores: Option<u32>,
    ) -> process::Command {
        let mut command = match build_command {
            BuildCommand::Install => {
                let mut c = process::Command::new("nixos-install");
                c.arg("--root").arg("/mnt").arg("--no-root-password");
                c
            }
            BuildCommand::Switch | BuildCommand::Boot | BuildCommand::BuildVm => {
                let mut c = process::Command::new("nixos-rebuild");
                c.arg(build_command.as_str());
                c
            }
        };
        command
            .arg("--flake")
            .arg(format!("{}#{}", path_config, config_name));
        if let Some(cores) = cores {
            command.arg("--cores").arg(cores.to_string());
        }
        command
    }

    /// Runs the NixOS rebuild in a subprocess and waits for it to finish.
    ///
    /// Depending on the `build_command` variant:
    /// * [`BuildCommand::Install`] → `nixos-install --root /mnt --no-root-password --flake …`
    /// * [`BuildCommand::Switch`] / [`BuildCommand::Boot`] / [`BuildCommand::BuildVm`] →
    ///   `nixos-rebuild <cmd> --flake …`. `build-vm` drops its `result` symlink in
    ///   the calling process's working directory (no `--out-link` is passed), so
    ///   the caller picks where it lands by setting its own cwd - deliberately
    ///   not the config repo, which git watches.
    ///
    /// Standard output is inherited (visible in the parent terminal); standard
    /// error is captured into `stderr` if provided.
    ///
    /// # Arguments
    /// * `path_config`   – Repository holding the flake to build.
    /// * `config_name`   – `nixosConfigurations` attribute to build.
    /// * `build_command` – Which rebuild to run.
    /// * `stderr`        – Buffer the child's stderr is appended to, for the
    ///   caller to put in a [`mx::ErrorKind::BuildError`]; pass `None` to
    ///   discard it.
    /// * `cores`         – forwarded as `--cores <cores>`; `None` leaves the
    ///   Nix default.
    ///
    /// # Returns
    /// `Ok(true)` if the process exited successfully (code 0), `Ok(false)` otherwise.
    ///
    /// # Post-conditions
    /// Blocks for the whole rebuild. On success with `Switch` the running system
    /// has already changed - this is the point of no return the rollback cannot
    /// undo by itself.
    ///
    /// # Errors
    /// [`mx::ErrorKind::IOError`] if the process cannot be spawned or waited
    /// for; a non-zero exit is reported as `Ok(false)`, not as an error.
    fn rebuild_config(
        path_config: &str,
        config_name: &str,
        build_command: BuildCommand,
        stderr: Option<&mut String>,
        cores: Option<u32>,
    ) -> mx::Result<bool> {
        let mut command =
            Self::build_rebuild_command(path_config, config_name, &build_command, cores);
        let mut child = command
            .stdout(process::Stdio::inherit())
            .stderr(process::Stdio::piped())
            .spawn()
            .map_err(mx::ErrorKind::IOError)?;

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

    /// Path of the repository's `flake.lock`.
    ///
    /// # Returns
    /// `<git_repo_path>/flake.lock`, whether the file exists or not.
    fn flake_lock_path(&self) -> path::PathBuf {
        path::Path::new(&self.git_repo_path).join("flake.lock")
    }

    /// Whether `flake.lock` physically exists in the repository directory.
    ///
    /// # Returns
    /// `true` when the file is there. If it is absent, a `nix flake update` is
    /// run before the commit to generate the initial lockfile.
    fn flake_lock_exists(&self) -> bool {
        self.flake_lock_path().exists()
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
    ///
    /// # Pre-conditions
    /// The transaction must be open, and the files to record must already be in
    /// the index (see [`Transaction::git_add`]).
    ///
    /// # Post-conditions
    /// The commit becomes the new tip of `update_ref`, and its OID is what a
    /// rollback moves away from.
    ///
    /// # Errors
    /// [`mx::ErrorKind::GitError`] on any libgit2 failure, and
    /// [`mx::ErrorKind::TransactionNotBegin`] outside an open transaction.
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

    /// Determines whether a file was modified since commit `oid`.
    ///
    /// Used in [`commit_impl`] to include only the actually modified files in the
    /// Git commit, avoiding empty commits.
    ///
    /// If `oid` is zero (empty repository), the file is always considered new.
    ///
    /// # Arguments
    /// * `repo`      – Repository to inspect.
    /// * `oid`       – Commit to compare against, usually the transaction's
    ///   `old_commit`.
    /// * `file_path` – Path of the file, relative to the repository root.
    ///
    /// # Returns
    /// `true` when the working tree or the index differs from `oid` for that
    /// file, or when the file is new.
    ///
    /// # Errors
    /// [`mx::ErrorKind::GitError`] if the file's status cannot be read.
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

    /// Adds a file to the Git index (equivalent to `git add <path>`).
    ///
    /// # Arguments
    /// * `path` – Path of the file, relative to the repository root.
    ///
    /// # Pre-conditions
    /// The transaction must be open; this panics otherwise, as it is only
    /// called from the commit path.
    ///
    /// # Post-conditions
    /// The index is written to disk, so the staged state survives a crash before
    /// the commit.
    ///
    /// # Errors
    /// [`mx::ErrorKind::GitError`] if the path cannot be staged or the index
    /// cannot be written.
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
    ///
    /// # Returns
    /// `true` between [`Transaction::begin`] and the commit or rollback that
    /// closes it.
    #[allow(dead_code)]
    pub fn as_begin(&self) -> bool {
        self.git_repo.is_some()
    }

    /// Returns a mutable reference to the [`NixFile`] associated with `path`.
    ///
    /// # Arguments
    /// * `path` – Path the file was registered under, as given to `add_file`.
    ///
    /// # Returns
    /// The open file, ready to be edited in memory.
    ///
    /// # Errors
    /// * `mx::ErrorKind::TransactionNotBegin` – `begin` has not been called yet.
    /// * `mx::ErrorKind::PermissionDenied`    – the transaction is read-only.
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
            .ok_or_else(|| mx::ErrorKind::FileNotFound(path.to_string()))
    }

    /// Returns a shared reference to the [`NixFile`] associated with `path`.
    ///
    /// # Arguments
    /// * `path` – Path the file was registered under.
    ///
    /// # Returns
    /// The open file, for reading only; allowed whatever the transaction's
    /// permission.
    ///
    /// # Errors
    /// * `mx::ErrorKind::TransactionNotBegin` – `begin` has not been called yet.
    /// * `mx::ErrorKind::FileNotFound`        – `path` was not added via `add_file`.
    pub fn get_file(&mut self, path: &str) -> mx::Result<&NixFile> {
        if self.git_repo.is_none() {
            return Err(mx::ErrorKind::TransactionNotBegin);
        }
        self.list_file
            .get(path)
            .ok_or_else(|| mx::ErrorKind::FileNotFound(path.to_string()))
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
    /// # Pre-conditions
    /// `git_repo_path` must be a git repository; a plain directory is refused.
    ///
    /// # Post-conditions
    /// Every registered file is locked until the commit or the rollback, so this
    /// blocks on a file another transaction holds. The caller's uncommitted work
    /// is stashed away and restored at the end. From here on, one of
    /// [`Transaction::commit`] or [`Transaction::rollback`] must be reached,
    /// otherwise the locks leak for the lifetime of the process.
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
                };

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
                    Err(mx::ErrorKind::FileNotFound(_))
                        if let TransactionPermission::Writtable = self.permission_transaction =>
                    {
                        file.create_file()?;
                        file.begin(NixFilePermission::Writtable)?;
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
                    git2::Oid::ZERO_SHA1
                }
                Err(e) => return Err(mx::ErrorKind::GitError(e)),
            };
        }
        if let TransactionPermission::Writtable = self.permission_transaction {
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
    ///
    /// # Post-conditions
    /// No stash entry of this transaction is left behind, and a conflicting
    /// stash is discarded rather than reported - the caller's uncommitted work
    /// is then lost, which is the price of keeping the repository usable. A
    /// no-op when `begin` had nothing to stash.
    ///
    /// # Errors
    /// [`mx::ErrorKind::TransactionNotBegin`] outside an open transaction.
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

    /// Refreshes `flake.lock`, per `update_input`.
    ///
    /// Generates the lockfile from scratch with a plain `nix flake update` if
    /// it does not exist yet; otherwise runs `nix flake update [inputs…]`
    /// (`UpdateInput::Keep` runs nothing).
    ///
    /// # Arguments
    /// * `update_input` – how `flake.lock` is refreshed.
    ///
    /// # `flake.lock` immutability
    /// When updating an existing lockfile, its immutable flag (set by `init`
    /// like every other base file) is cleared first, since `nix flake update`
    /// rewrites it in place, and restored right after - but only if it *was*
    /// set: on a config repo not produced by `init` the file is writable on
    /// purpose, and sealing it here would permanently break the admin's own
    /// `nix flake update`. This is a no-op on a file that is not root-owned
    /// (dev checkouts). The `nix` command's own result is checked before the
    /// re-seal's, so a failure to re-seal never masks the actual `nix flake
    /// update` error.
    ///
    /// # Errors
    /// [`mx::ErrorKind::IOError`] if `nix` cannot be spawned or waited for,
    /// [`mx::ErrorKind::InvalidFile`] if the lockfile path is not UTF-8.
    fn run_flake_update(&self, update_input: UpdateInput) -> mx::Result<()> {
        if !self.flake_lock_exists() {
            process::Command::new("nix")
                .args(["flake", "update"])
                .current_dir(&self.git_repo_path)
                .output()
                .map_err(mx::ErrorKind::IOError)?;
            return Ok(());
        }

        let mut args = vec!["flake".to_string(), "update".to_string()];
        let command = match update_input {
            UpdateInput::UpdateAll => args,
            UpdateInput::UpdateSelected(s) => {
                args.extend(s);
                args
            }
            UpdateInput::Keep => vec![],
        };
        if command.is_empty() {
            return Ok(());
        }

        let lock_path = self.flake_lock_path();
        let lock_path = lock_path.to_str().ok_or(mx::ErrorKind::InvalidFile)?;
        let was_immutable = NixFile::is_immutable(lock_path)?;
        NixFile::make_mutable(lock_path)?;
        let result = process::Command::new("nix")
            .args(command)
            .current_dir(&self.git_repo_path)
            .output()
            .map_err(mx::ErrorKind::IOError);
        let resealed = if was_immutable {
            NixFile::make_immutable(lock_path)
        } else {
            Ok(())
        };
        result?;
        resealed
    }

    /// Internal commit implementation, split out so the [`commit`] wrapper can
    /// trigger an automatic rollback on failure.
    ///
    /// Steps:
    /// 1. Commit each [`NixFile`] to disk.
    /// 2. Detect the actually modified files (selective `git add`).
    /// 3. If at least one file changed, or [`force_commit`](Self::set_force_commit)
    ///    is set: refresh `flake.lock` via [`run_flake_update`]; a forced
    ///    refresh that touched no file only proceeds to the commit if that
    ///    refresh actually moved `flake.lock`.
    /// 4. If a commit is warranted: create the Git commit, then enter the FIFO
    ///    build queue and, once at the head, run `nixos-rebuild`.
    /// 5. Close all [`NixFile`]s and release the Git repository.
    ///
    /// # Arguments
    /// * `update_input` – How `flake.lock` is refreshed before the commit.
    ///
    /// # Post-conditions
    /// A commit is only created when a file genuinely changed (or a forced
    /// refresh moved `flake.lock`), so an operation that changes nothing
    /// leaves no empty commit and runs no rebuild. The rebuild is serialised
    /// against the other processes' rebuilds, so the call can block well
    /// beyond its own build time. The transaction is closed on success: the
    /// files are unlocked and the repository handle is dropped.
    ///
    /// # Errors
    /// [`mx::ErrorKind::BuildError`] with the rebuild's stderr,
    /// [`mx::ErrorKind::GitError`], [`mx::ErrorKind::IOError`], or
    /// [`mx::ErrorKind::TransactionNotBegin`] outside an open transaction. On
    /// error the files may already have been written to disk - it is
    /// [`Transaction::commit`] that turns that into a rollback.
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

        if need_modif || self.force_commit {
            self.run_flake_update(update_input)?;
            if !need_modif {
                need_modif = self.flake_lock_modified()?;
            }
        }

        if need_modif {
            self.git_commit(Some("HEAD"), &self.git_user, &self.git_user, &self.info)?;

            let skip = LockFile::try_lock(LOCK_SKIP_REBUILD_FILE)?;
            if let Some(mut sentinel) = skip {
                sentinel.unlock();

                let ticket = BuildQueue::enqueue()?;
                ticket.wait_turn()?;

                let mut stderr = String::new();
                let success = Self::rebuild_config(
                    &self.git_repo_path,
                    CONFIG_NAME,
                    self.build_type.clone(),
                    Some(&mut stderr),
                    self.rebuild_cores,
                )?;
                if !success {
                    return Err(mx::ErrorKind::BuildError(stderr));
                }
            }
        }

        for (_, nix_file) in self.list_file.iter_mut() {
            nix_file.close()?;
        }
        self.stash_restore()?;
        self.git_repo = None;
        Ok(())
    }
    /// Persists the changes, creates a Git commit and triggers the NixOS rebuild.
    ///
    /// On internal failure, an automatic [`rollback`] is attempted before
    /// propagating the error.
    ///
    /// # Arguments
    /// * `update_input` – How `flake.lock` is refreshed before the commit.
    ///
    /// # Pre-conditions
    /// The transaction must be open and writable.
    ///
    /// # Post-conditions
    /// On success the change is committed to git and, unless nothing changed,
    /// applied by the rebuild. On failure the configuration is back to
    /// `old_commit` and the files are unlocked. A `Switch` rebuild that fails
    /// midway is the one case the rollback cannot fully undo: the configuration
    /// is restored, but whatever the rebuild already did to the running system
    /// is not. Blocks for the whole rebuild.
    ///
    /// # Errors
    /// As in [`Transaction::commit_impl`]; the rollback's own error, if any, is
    /// swallowed so the original cause is the one reported.
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
    ///    files and return without touching Git. There is nothing to check out,
    ///    but the files `begin` created still have to go - otherwise a failed
    ///    first transaction (typically `init`, whose repo is unborn by
    ///    construction) leaves the empty `{ }` skeletons behind and the next run
    ///    sees a half-populated config directory.
    /// 2. Otherwise: repoint the current branch to `old_commit` and perform a
    ///    `checkout --force` to restore the working tree.
    /// 3. Remove the files created during the transaction; re-apply the immutable
    ///    flag on the restored pre-existing files.
    ///
    /// # Immutable flag vs. `checkout_head`
    /// `flake.lock` is rewritten and re-sealed during `commit_impl` but no
    /// `NixFile` owns it, so it is made mutable separately before the checkout;
    /// every tracked file is made mutable for the same reason. Left immutable,
    /// `checkout_head` fails with `EPERM` while restoring the previous file and
    /// the rollback aborts with HEAD already moved.
    ///
    /// # Post-conditions
    /// Every [`NixFile`] is closed - which is mandatory, since a leaked flock
    /// would block every later `begin` - the immutable flags are back, and the
    /// stash `begin` took is restored. The repository handle is dropped, so the
    /// transaction cannot be reused.
    ///
    /// # Errors
    /// `mx::ErrorKind::TransactionNotBegin` if no transaction is active, plus
    /// [`mx::ErrorKind::GitError`] or [`mx::ErrorKind::IOError`] if the previous
    /// state cannot be restored.
    pub fn rollback(&mut self) -> mx::Result<()> {
        if self.git_repo.is_none() {
            return Err(mx::ErrorKind::TransactionNotBegin);
        }

        {
            if self.old_commit.is_zero() {
                for (_, nix_file) in self.list_file.iter_mut() {
                    let _ = nix_file.close();
                    if nix_file.was_created() {
                        NixFile::make_mutable(nix_file.get_file_path()).ok();
                        std::fs::remove_file(nix_file.get_file_path()).ok();
                    }
                }
                self.git_repo = None;
                return Ok(());
            }

            let flake_lock = self.flake_lock_path();
            let flake_lock = flake_lock.to_str().unwrap_or_default().to_owned();
            let flake_lock_sealed =
                !flake_lock.is_empty() && NixFile::is_immutable(&flake_lock).unwrap_or(false);
            if flake_lock_sealed {
                NixFile::make_mutable(&flake_lock).ok();
            }

            let repo = self.git_repo.as_ref().unwrap();
            let head = repo.head().map_err(mx::ErrorKind::GitError)?;

            let refname = head.name().map_err(|_| {
                mx::ErrorKind::GitError(git2::Error::from_str("HEAD is not a symbolic ref"))
            })?;

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
            if flake_lock_sealed && path::Path::new(&flake_lock).exists() {
                NixFile::make_immutable(&flake_lock).ok();
            }

            for (_, nix_file) in self.list_file.iter_mut() {
                let _ = nix_file.close();
            }
        }
        self.stash_restore()?;
        self.git_repo = None;
        Ok(())
    }
}

#[cfg(test)]
#[path = "transaction_tests.rs"]
mod tests;
