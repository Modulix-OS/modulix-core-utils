/// Tests for the enabled-module listing.
///
/// `list_enabled_module_names` opens a read-only transaction on a real
/// `module.nix`, so every test here works on a temporary git repository, the
/// same fixture shape `core/transaction/transaction_tests.rs` uses.
///
/// ```
/// cargo test --features install-module install_module
/// ```
use super::list_enabled_module_names;
use std::fs;
use tempfile::TempDir;

/// Repo path with the trailing `/` `NixFile` expects.
fn repo_path(dir: &TempDir) -> String {
    format!("{}/", dir.path().to_str().unwrap())
}

/// A committed git repo holding `module.nix` with `content`. `configuration.nix`
/// is written too: `Transaction::begin` adds it to every transaction.
fn setup_repo(module_nix: Option<&str>) -> TempDir {
    let dir = TempDir::new().expect("failed to create temporary directory");
    let repo = git2::Repository::init(dir.path()).expect("git init failed");
    fs::write(
        dir.path().join("configuration.nix"),
        "{config, lib, pkgs, ...}:\n{\n  imports = [ ./module.nix ];\n}\n",
    )
    .expect("failed to write configuration.nix");
    if let Some(content) = module_nix {
        fs::write(dir.path().join("module.nix"), content).expect("failed to write module.nix");
    }

    let sig = git2::Signature::now("Test", "test@test.com").unwrap();
    let mut index = repo.index().unwrap();
    index
        .add_all(["*"].iter(), git2::IndexAddOption::DEFAULT, None)
        .unwrap();
    index.write().unwrap();
    let tree_oid = index.write_tree().unwrap();
    let tree = repo.find_tree(tree_oid).unwrap();
    repo.commit(Some("HEAD"), &sig, &sig, "init", &tree, &[])
        .unwrap();
    dir
}

/// The shape `mx-daemon` writes: a module name is a dotted path several
/// segments deep, and all of it must come back.
#[test]
fn lists_nested_enabled_module() {
    let dir = setup_repo(Some(
        r#"{config, lib, pkgs, ...}:
{
  mx = {
    programs = {
      studio = {
        obs-studio = {
          enable = true;
        };
      };
    };
  };
}
"#,
    ));
    assert_eq!(
        list_enabled_module_names(&repo_path(&dir)).unwrap(),
        vec!["programs.studio.obs-studio"]
    );
}

/// The same module written flat, plus a single-segment one.
#[test]
fn lists_flat_and_single_segment_modules() {
    let dir = setup_repo(Some(
        "{config, lib, pkgs, ...}:\n{\n  mx.programs.studio.obs-studio.enable = true;\n  mx.fonts.enable = true;\n}\n",
    ));
    let mut names = list_enabled_module_names(&repo_path(&dir)).unwrap();
    names.sort();
    assert_eq!(names, vec!["fonts", "programs.studio.obs-studio"]);
}

/// `enable = false` declares the module without enabling it.
#[test]
fn skips_disabled_module() {
    let dir = setup_repo(Some(
        "{config, lib, pkgs, ...}:\n{\n  mx.programs.studio.obs-studio.enable = false;\n  mx.fonts.enable = true;\n}\n",
    ));
    assert_eq!(
        list_enabled_module_names(&repo_path(&dir)).unwrap(),
        vec!["fonts"]
    );
}

/// A module listed only through its plugins is not enabled.
#[test]
fn plugins_alone_do_not_enable_a_module() {
    let dir = setup_repo(Some(
        "{config, lib, pkgs, ...}:\n{\n  mx.programs.studio.obs-studio.plugins = [ pkgs.obs-studio-plugins.wlrobs ];\n}\n",
    ));
    assert!(
        list_enabled_module_names(&repo_path(&dir))
            .unwrap()
            .is_empty()
    );
}

/// A configuration that never had a module installed has no `module.nix`:
/// "no module enabled", not an error.
#[test]
fn missing_module_file_yields_empty_list() {
    let dir = setup_repo(None);
    assert_eq!(
        list_enabled_module_names(&repo_path(&dir)).unwrap(),
        Vec::<String>::new()
    );
}
