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
- **`BuildCommand::as_str()`** (`transaction.rs`): in debug **all** variants (`Switch`/`Boot`/`Install`) return `"build-vm"`, so a commit never touches the host system — it builds a VM image instead.

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
  3. if `need_modif`: generate `flake.lock` via `nix flake update` if absent, else honor `UpdateInput` (`Keep`=no update / `UpdateAll` / `UpdateSelected(inputs)`); `flake.lock` is auto-staged if modified;
  4. create the git commit (parentless if repo was empty);
  5. **build serialization** via two file locks: `try_lock(/tmp/mx-queue-build.lock)` — only if acquired do we `lock(/tmp/mx-build.lock)`, release the queue lock, then run the rebuild. This lets a single waiter coalesce concurrent builds;
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

### Other subsystems

- **`detect_hardware/`** (`detect-hardware`): parses `lspci`/`lsusb`/`cpuid` with regex → CPU/GPU/machine info → derives Nix driver config. The wrapped binary needs `pciutils`/`usbutils`/`cpuid` on PATH (`flake.nix postInstall` wraps it).
- **`package_info/`** (`package-info`): `NixPackage` (serde) + lazy flatpak/flathub resolution (`OnceCell`, reqwest, tokio).
- **`desktop_environment/`** (`desktop-environment`): writes GNOME (dconf/gtk) and Plasma configs.
- **`config_store/`**: key/value store.

### Cross-cutting conventions

- **Errors**: one enum `error::ErrorKind`, re-exported as `crate::mx::{Result, ErrorKind}`. No `thiserror`. Return `mx::Result<T>`, propagate with `?`.
- **Renamed imports**: `Option as mxOption`, `List as mxList` to avoid shadowing std types.
- Never hand-edit Nix as raw strings in domain code — go through `mxOption`/`mxList` so AST positioning and indentation stay correct.

## Adding a new config module

1. Declare the feature in `Cargo.toml` (depend on at least `core-nix-file`).
2. Create `src/<module>.rs` with the `*_no_transaction` + `make_transaction` wrapper pair and a `FILE_*_PATH` constant.
3. Register it in `lib.rs` under `#[cfg(feature = "<module>")]`.
4. Manipulate config via `mxOption`/`mxList` only.
5. Add an `examples/<module>.rs` and test with `--features <module>`.
