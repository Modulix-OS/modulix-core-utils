# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Overview

`modulix-core-utils` is a Rust library (edition 2024) for Modulix OS (a NixOS-based distro). It exposes a typed API to **programmatically edit Nix configuration files** and drive `nixos-rebuild`, all inside atomic, locked, git-versioned transactions.

Each configuration domain (firewall, locale, users, packages, modules…) is an optional **feature-gated module**. Nothing compiles unless its feature is enabled. **All code comments and doc comments are in English** — keep new comments in English.

## Commands

No README. Always pass `--features`; the library builds nothing by default.

```bash
# Build / check one feature
cargo build --features firewall
cargo clippy --features install-package -- -D warnings   # clippy required (global rules)
cargo fmt

# Tests: nearly all live in src/core/transaction/*_tests.rs
cargo test --features modulix-module --all-targets        # any core-nix-file feature pulls in the transaction tests
cargo test --features <feat> <name>                       # single test by name filter

# Examples: each one needs ITS feature. The Cargo.toml [[exemple]] keys are a typo
# for [[example]] and are silently ignored by cargo, so required-features does NOT apply
# — pass --features by hand (see .zed/debug.json for the canonical per-example flags).
cargo run --example firewall       --features firewall
cargo run --example package        --features install-package
cargo run --example search_package --features "package-info,app-info-gui"

# Generator binary (regenerates the flathub_basic_info.rs lookup table)
cargo run --bin flathub-info-gen --features flathub-info-gen

# Nix
nix develop          # devShell: cargo, rustc, rustfmt, openssl, pciutils, usbutils, cpuid
nix build .#debug    # non-release build via naersk
```

`package-info-full` appears in `.zed/` but **does not exist** in `Cargo.toml`. To match the IDE setup, enable `package-info,app-info-gui,install-package,init`.

## Debug vs release behavior (critical)

`#[cfg(debug_assertions)]` changes two things that make debug builds safe to run on a dev host:

- **`CONFIG_DIRECTORY`** (`lib.rs`): `/etc/modulix-os/` in release, `<repo>/test/` in debug. `test/` is a **real NixOS fixture config** (`configuration.nix`, `firewall.nix`, `users.nix`, `fstab.nix`…) that examples/tests mutate in debug. It is also a nested git repo (`test/.git`) because the transaction engine requires one.
- **`BuildCommand::as_str()`** (`transaction.rs`): in debug every variant that would touch the host (`Switch`/`Boot`/`Install`/`BuildVm`) returns `"build-vm"`, so a commit never touches the host system — it builds a VM image instead. `Build` is the exception: it activates nothing, so it stays `"build"` in both profiles.

`build.rs` injects `TARGET_NIX` (e.g. `x86_64-linux`) as a compile-time env var.

## Architecture

### The transactional core (`src/core/`, feature `core-nix-file`)

Everything routes through here. Understand it first. Two stacked lock/atomicity layers: per-file (`NixFile`) and per-build/repo (`Transaction`).

#### `transaction/mod.rs` — entry points

- `make_transaction(desc, config_dir, file_path, build_command, update_input, closure)` is the **standard entry point** every domain module uses: builds a `Transaction`, `add_file` + `begin`, runs the closure on a `&mut NixFile`, then **`commit` on `Ok`, `rollback` on `Err`** (and on `get_file` failure). `make_transaction_read_only` is the `ReadOnly` variant.

#### `transaction/file_lock.rs` — `NixFile`, the per-file atomic unit

In-memory edit buffer with atomic write-on-commit and immutable-flag handling:

- State: `file: Option<File>`, `file_content: String`, `was_created: bool`, `writable: bool`. Only instantiable inside a transaction.
- `begin(permission)`: if writable, **clears the ext2 immutable flag** (`make_mutable`), opens RW, takes an **exclusive `flock`**, reads whole file into `file_content`.
- Edits happen purely in `file_content` via `get_mut_file_content()` (errors `PermissionDenied` if read-only, `TransactionNotBegin` if not open).
- `commit()`: seek 0 → `set_len(0)` → write full buffer → **re-apply immutable flag** → release lock → reset. Single rewrite, so other processes never see a partial file.
- `close()`: release lock + reset **without** writing (discards edits); re-applies immutable flag if it was writable.
- `create_file()`: writes the stub `{config, lib, pkgs, ...}:\n{\n}\n`, marks `was_created`, sets immutable.
- Immutable flag is toggled via raw `ioctl` `FS_IOC_GETFLAGS`/`SETFLAGS` with `FS_IMMUTABLE_FL` (each `unsafe` block carries a `// SAFETY`-style rationale). Flag is **only applied to root-owned files** (`is_owned_by_root`), so it no-ops on the dev fixture.

#### `transaction/transaction.rs` — `Transaction`, the per-build atomic unit

Wraps a set of `NixFile`s over a **git2** repo. Key fields: `list_file: HashMap<path, NixFile>`, `git_repo: Option<Repository>` (Some ⟺ active), `old_commit: Oid` (rollback target captured at `begin`), `stash_oid: Option<Oid>` (auto-stash restore point), `build_type`, `permission_transaction`.

Lifecycle:
- **`new`** — pure constructor, no I/O. Author/committer signature hardcoded to `Modulix-OS <modulix.os@ik-mail.com>`.
- **`add_file(path)`** — register a file; must be called **before** `begin` (else `TransactionAlreadyBegin`).
- **`begin()`** —
  1. auto-adds `configuration.nix`;
  2. opens the git repo;
  3. if the worktree is dirty (incl. untracked), **`stash_save(INCLUDE_UNTRACKED)`** so the transaction works on a clean tree, restored at the end;
  4. `NixFile::begin` on each file; a missing file in `Writtable` mode is **created** and queued to be added to the `imports` list of `configuration.nix`;
  5. captures `old_commit = HEAD` (`Oid::ZERO` if repo unborn/empty).
- **`commit(update_input)` → `commit_impl`** —
  1. `NixFile::commit()` each file to disk;
  2. `has_diff_with_commit(old_commit, path)` selects only genuinely changed files and `git_add`s them (avoids empty commits) → `need_modif`;
  3. if `need_modif`: generate `flake.lock` via `nix flake update` if absent, else honor `UpdateInput` (`Keep`=no update / `UpdateAll` / `UpdateSelected(inputs)` / `UseLock(content)`=write that exact lockfile, no `nix` process); `flake.lock` is auto-staged if modified;
  4. create the git commit (parentless if repo was empty);
  5. **build serialization** via `BuildQueue` (`core/transaction/build_queue.rs`): a FIFO ticket directory, `/tmp/mx-build-queue`, where each waiter holds an exclusive flock on its ticket file as a liveness proof — `wait_turn` deletes the tickets whose flock is free, so a killed process never wedges the queue. Separately, the sentinel `/tmp/mx-skip-rebuild.lock` is probed with `try_lock`: held by someone else ⇒ the commit stands and **no rebuild runs** (`init` uses that so an install does not rebuild per intermediate transaction). `Transaction::set_skip_rebuild(true)` is the explicit, per-transaction form of the same thing, used by the staged-update promotion;
  6. `rebuild_config`: `Install` → `nixos-install --root /mnt --no-root-password --flake <dir>#<CONFIG_NAME>`; `Switch`/`Boot` → `nixos-rebuild <cmd> --flake …`. stdout inherited, stderr captured; non-zero exit → `BuildError(stderr)`;
  7. `NixFile::close()` all, `stash_restore()`, drop the repo handle. Any failure inside `commit_impl` triggers an automatic `rollback`.
- **`rollback()`** — if `old_commit` is zero just close files; otherwise repoint HEAD ref to `old_commit`, force `checkout_head`, **delete files that `was_created`**, restore immutable flag on pre-existing ones, `close()` every `NixFile` (mandatory — otherwise the flock leaks and blocks future `begin`s), then `stash_restore`.

> Invariant: the config repo must be clean-committable. Untracked/uncommitted files that can't be stashed surface as `ErrorKind::GitNotCommitted`.

#### `option.rs` — `Option`, single-value Nix options

Reads/writes an option by dotted path (`"services.nginx.enable"`). Parses the file with **rnix/rowan** into an AST, then:
- `set`: on `ExistingOption`, replace just the value range; on `NewInsertion`, synthesize the missing nested `key = { … };` braces with correct indentation (`TABULATION_SIZE = 2` spaces per level) and splice at the insertion point.
- `get`, `set_option_to_default` (removes the option + surrounding whitespace), `set_option_all_instance_to_default` (loops until none remain).

#### `localise_option.rs` — AST localization

`SettingsPosition::new(ast, dotted_path)` walks `AttrSet`/`AttrpathValue` nodes and returns either `ExistingOption { range_path, range_value, indent_level }` or `NewInsertion { pos, rest_option_path, indent_level }` (deepest existing prefix + remaining path to create). All positions are byte `Range`s into the source, which `Option`/`List` splice directly.

#### `list.rs` — `List`, Nix list options

For list-valued options (`environment.systemPackages`, `networking.firewall.allowedTCPPorts`, `imports`…). `new(path, unique_value)`; `add` (skips duplicates when `unique_value`, creates `[]` then recurses if absent), `remove` (collapses to default when emptying the last element), plus `eq`/`countains`/`get_element_in_list`. Built on top of `Option` — it edits the rendered list text and writes it back through `Option::set`.

#### `app_info_trait/` (feature `core-app-info-trait`)

Async traits `AppInfoMinimal` / `AppInfoGui` abstracting app-info sources, plus `PLUGIN_NAMESPACES` (a `phf` map) linking a package name to its Nix options (`programs.X.enable`, plugin-list paths). `install_package` consults this map to decide between `programs.X.enable = true` and appending to `environment.systemPackages`.

### Domain modules (`src/*.rs`, one feature each)

All follow the **same two-level pattern** (`firewall.rs`, `modulix_modules.rs` are the cleanest references):
1. `*_no_transaction(file: &mut NixFile, …)` — pure, composable edit using `mxOption`/`mxList`.
2. public wrapper `add_x(config_dir, …)` that wraps the `_no_transaction` fn in `make_transaction` with a constant `FILE_*_PATH` (`firewall.nix`, `package.nix`, `modules.nix`…).

Modules: `firewall`, `locale`, `user`, `filesystem`, `flake_input`, `init`, `hardware_config`, `modulix_modules`, `install_package`.

**`install_package` and `install_module` are each split across two features**, because `init` needs only their composable half while their public wrappers drag in half the crate (`install-package` → `package-info` → `memmap2`; `install-module` → `module-info` → `reqwest` + TLS). The light `install-package-file` / `install-module-file` are `["core-nix-file"]` and expose only `FILE_*_PATH` (both `pub`, like `locale::LOCALE_FILE_PATH`) and the `*_no_transaction` fns; everything else — `install`/`uninstall`, the `list_*` readers, the index fetch, and each module's `mod tests` — stays under the heavy feature. `init` depends on the two light ones, so an installer linking `init` pulls no HTTP client. `src/lib.rs` gates both modules on `any(heavy, light)`.

#### `filesystem.rs` and LUKS

**All LUKS knowledge lives here, not in callers.** `add_mount_no_transaction` takes a `device: &MountDevice`, which is either `Plain { device }` or `Luks { container, mapper_device, tpm2 }`. A caller states only facts it can observe — "this is a LUKS volume, here is the container's `by-uuid` path and the `/dev/mapper/…` it is open as" — and never names a mapper, spells a `boot.initrd.luks.devices` path, or builds a `LuksEntry`. `LuksEntry`, `default_luks_name` and `mapper_name` are private.

`mapper_name(mapper_device, container)` takes the basename of `mapper_device` when it is under `/dev/mapper/`, so the declaration matches the name the container is really opened as; it falls back to `default_luks_name(container)` → `luks-<uuid>`. A `Luks` container that is not a `/dev/disk/by-uuid/` path is refused with `InvalidUuid`: the initrd has no stable device names.

`MountDevice::Luks::tpm2 = false` means "do not add it", **not** "remove it". `add_mount_no_transaction` resets only `{root}.options`, never `crypttabExtraOpts`, so an existing enrolment survives. That is what lets `mx-daemon` — which reads `fstab` entries, and an `fstab` entry says nothing about TPM2 — always pass `false` without destroying one.

`remove_mount_no_transaction` also drops the `boot.initrd.luks.devices."<name>"` entry, **but only when no other remaining mount point still mounts that same `/dev/mapper/<name>`**: sub-volumes and binds share one container, and dropping the entry while one of them survives leaves an initrd unable to open it. `mapper_still_used` enumerates with `mxOption::list_children("fileSystems")`, which hands back each key as written — quotes included — so it composes straight into a dotted path. `drop_luks_entry` removes the entry's own path *and* its `.device`/`.crypttabExtraOpts` leaves: `add_mount_no_transaction` writes the entry as nested attribute sets, `nixos-generate-config` writes each leaf flat, and removing a parent path does not match a flat leaf.

Note `option::get`'s documentation points at `core::utils::string_nix_to_value` to unquote a value it returns — **that module is declared nowhere in `core/mod.rs`, so it is not compiled**. `mounted_mapper` strips the quotes itself.

`set_luks_tpm2_no_transaction(fstab, luks_name)` exists on its own because **`nixos-generate-config` already declares the LUKS entry's `.device`** — it detects the open mapper and keys the entry on the live `/sys/class/block/<dm>/dm/name` — and `fstab_module` copies that block into `fstab.nix` verbatim (`extract_fs_block` preserves it on purpose). A caller that lets the generator describe the encrypted root must therefore add only `crypttabExtraOpts` and must **not** re-declare `.device`: two definitions of the same `types.str` option make the NixOS module system fail, even at equal values. Neither function writes `boot.initrd.systemd.enable`/`.tpm2.enable` — in Modulix those come from `mxpkgs/modulixos/boot.nix`.

`set_luks_container_no_transaction(fstab, luks_name, container)` is the exception to that rule, and only because the generator has a blind spot: it emits `boot.initrd.luks.devices."<name>".device` **only while walking the mount points it found**, so a container holding swap alone — which it reports under `swapDevices` and nowhere else — arrives with no way to be unlocked in the initrd. That function declares the missing `.device`, refuses anything that is not a `/dev/disk/by-uuid/` path, and must never be aimed at a container the generator already described.

`set_resume_device_no_transaction(fstab, device)` writes `boot.resumeDevice`. It lives in `fstab.nix` next to the swap device it names, and it is not optional for hibernation: with a systemd initrd NixOS only passes `resume=` when that option is set (`nixos/modules/system/boot/systemd/initrd.nix`), the fallback over `swapDevices` exists in the script initrd only. The value renders as a `boot = { resumeDevice = …; }` attribute set, which Nix merges with the flat `boot.initrd.…` lines the generator wrote.

Known gap: `def_filesystem_from_unix_fstab_no_transaction` replaces the whole file with generator output, so it destroys `crypttabExtraOpts`, any `.device` written by `set_luks_container_no_transaction`, and `boot.resumeDevice`. `extract_fs_block` cannot help — it never reads the existing file.

#### `init.rs`

`InitParams` carries four fields beyond the identity/locale seed, all of them optional:

- `packages: Vec<String>` → `package.nix` via `install_package::install_no_transaction`, so the entries carry the `pkgs.<attr>` spelling a later `install_package::uninstall` matches on.
- `modules: Vec<String>` → `module.nix` via `install_module::install_no_transaction`, one `mx.<name>.enable = true` per dotted name (as spelled in mxpkgs' `modules/index.json`; not validated, an unknown name fails at build time).
- `luks: Vec<LuksInit>` → `write_fstab_extras` on `fstab.nix`, applied **after** the content write that replaces that buffer. One entry per LUKS container: `container: Some(path)` declares the `.device` the generator left out (a swap-only container), `None` means the generator already declared it (the root), and `tpm2` adds `crypttabExtraOpts`. An entry with `container: None` and `tpm2: false` writes nothing.
- `resume_device: Option<String>` → `filesystem::set_resume_device_no_transaction`, in the same pass. `/dev/mapper/<name>` when the swap lives in a LUKS container.

All four live in the same transaction as the rest of the seed, so the repo is one commit and — with the skip-rebuild lock held — no build. An empty list creates no file and adds no import. `configuration_nix` lists `./package.nix`/`./module.nix` by hand for exactly the reason the other four are listed: `Transaction::begin` does inject new files into `imports`, but `init` then overwrites `configuration.nix` wholesale. Neither file is in `BASE_FILES` — they are the two files a user keeps editing afterwards, and `NixFile::create_file`/`commit` seal them anyway.

### Other subsystems

- **`detect_hardware/`** (`detect-hardware`): parses `lspci`/`lsusb`/`cpuid` with regex → CPU/GPU/machine info → derives Nix driver config. The wrapped binary needs `pciutils`/`usbutils`/`cpuid` on PATH (`flake.nix postInstall` wraps it).
- **`package_info/`** (`package-info`): `NixPackage` (serde) + lazy flatpak/flathub resolution (`OnceCell`, reqwest, tokio).
- **`desktop_environment/`** (`desktop-environment`): writes GNOME (dconf/gtk) and Plasma configs.
- **`config_store/`**: key/value store.
- **`update.rs`** (`system-update`): the system-update surface, in two styles. Read side, async: `outdated_inputs(config_dir)` probes each direct input with `nix flake metadata --refresh` (one subprocess per input); `check_update(config_dir)` instead lets nix resolve the *whole* lockfile via `nix flake update --output-lock-file <scratch>` — nothing under `config_dir` is written — returning `Some(new_flake_lock_text)` when it differs from the current one, and `diff_locks(old, new)` describes that candidate locally (no process, no network). Write side, blocking: `update(...)` re-resolves inside the transaction (`UpdateInput::UpdateAll`); `update_with_lock(..., lock, ...)` writes a candidate from `check_update` verbatim (`UpdateInput::UseLock`) — what the daemon drives, so the revisions installed are the ones announced and the refresh is paid for once.

### Cross-cutting conventions

- **Errors**: one enum `error::ErrorKind`, re-exported as `crate::mx::{Result, ErrorKind}`. No `thiserror`. Return `mx::Result<T>`, propagate with `?`.
- **Renamed imports**: `Option as mxOption`, `List as mxList` to avoid shadowing std types.
- Never hand-edit Nix as raw strings in domain code — go through `mxOption`/`mxList` so AST positioning and indentation stay correct.

## Staged system updates (`staging.rs` + `bin/apply-update.rs`)

**A system update is never applied while the machine is in use.** It is resolved and built up front, and the switch happens at shutdown. State lives under `cache_dir()/pending-update/`: `config/` (the `HEAD` tree plus the candidate `flake.lock`), `lock` (the candidate, canonical), `result` (the `nixos-rebuild build` symlink, i.e. the **garbage-collection root** that keeps the pre-built closure alive), `meta.json` (`StagedUpdate`).

- `stage_update(config_dir, cores)` — `check_update` → stage the tree → `nixos-rebuild build` (on `spawn_blocking`, behind a `BuildQueue` ticket). Idempotent: an unchanged candidate whose closure is still rooted is returned as-is. Touches `config_dir` not at all.
- `apply_staged(config_dir, cores)` — `nixos-rebuild boot` **against the staged copy**, then promotes the candidate with `make_transaction_commit_only` (commit, no rebuild). It must be the staged copy: a `.git`-less checkout hashes as a `path:` flake where the real repository is `git+file://`, so activating the real repository would rebuild the top-level derivation instead of reusing the pre-built one.
- `staged_status()` / `discard_staged()` — read side and teardown. `built` is reported `false` once `result` is gone, so a caller is never promised a fast activation that would in fact rebuild.
- `repair_after_crash(config_dir)` → `Transaction::restore_orphan_stashes` — pops the auto-stash an interrupted transaction left behind. `Transaction` has no `Drop`, so a `SIGKILL` mid-transaction leaves one; nothing else detects it. Callers run it at start-up, before serving.
- `bin/apply-update.rs` builds the `mx-apply-update` binary (flake output `packages.<system>.mx-apply-update`), which mxpkgs runs from a `Before=shutdown.target` unit. It needs `/nix/store` and `/boot` still mounted, hence `DefaultDependencies=no` + `Before=umount.target`.

**Invariant: the committed `flake.lock` is the one the running system was built from.** The candidate stays out of the git tree, so a concurrent install (`UpdateInput::Keep`, which runs no `nix` process at all when a lockfile is present) cannot drag the pending update in.

Two related changes in `transaction.rs`: the rebuild is wrapped in `systemd-run --collect --wait --pipe --unit=mx-rebuild-<pid>-<n> --property=KillMode=process` when `INVOCATION_ID` is set, so it runs in its own cgroup and survives a stop of the calling unit (a daemon restarted by its own `switch` used to kill it); and `build_with_lock`'s scratch moved from `/tmp` to `cache_dir()`, because a transient unit does not see the `PrivateTmp` of the caller.

## Adding a new config module

1. Declare the feature in `Cargo.toml` (depend on at least `core-nix-file`).
2. Create `src/<module>.rs` with the `*_no_transaction` + `make_transaction` wrapper pair and a `FILE_*_PATH` constant.
3. Register it in `lib.rs` under `#[cfg(feature = "<module>")]`.
4. Manipulate config via `mxOption`/`mxList` only.
5. Add an `examples/<module>.rs` and test with `--features <module>`.
