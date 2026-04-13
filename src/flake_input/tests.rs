use super::{
    FlakeInput, remove_follower_no_transaction, remove_input_no_transaction,
    set_follower_no_transaction,
};
use crate::core::transaction::{self, transaction::BuildCommand};
use git2::Repository;
use std::fs;
use tempfile::tempdir;

fn create_flake_file(content: &str) -> (tempfile::TempDir, String) {
    let dir = tempdir().expect("failed to create temp dir");
    let path = dir.path().to_str().unwrap().to_string();
    Repository::init(&path).expect("failed to init git repo");
    let file_path = format!("{}/flake.nix", path);
    fs::write(&file_path, content).expect("failed to write flake.nix");
    (dir, path)
}

fn lock_build_queue() -> fs::File {
    let f = fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .open("/tmp/mx-queue-build.lock")
        .expect("failed to create build-queue lock file");
    f.lock().expect("failed to lock build-queue lock file");
    f
}

#[test]
fn add_follower_creates_follows_option() {
    let (_dir, path) = create_flake_file("{ config, lib, pkgs, ... }:\n{\n}\n");
    let _guard = lock_build_queue();
    transaction::make_transaction(
        "add follower",
        &format!("{}/", path),
        "flake.nix",
        BuildCommand::Switch,
        |file| set_follower_no_transaction(file, "foo", FlakeInput::Nixpkgs),
    )
    .unwrap();

    let content = fs::read_to_string(format!("{}/flake.nix", path)).unwrap();
    assert!(content.contains("follows = \"nixpkgs\""));
}

#[test]
fn remove_follower_deletes_follows_option() {
    let (_dir, path) = create_flake_file(
        "{ config, lib, pkgs, ... }:\n{\n  inputs.foo = {\n    url = \"github:example/repo\";\n    follows = \"nixpkgs\";\n  };\n}\n",
    );
    let _guard = lock_build_queue();
    let removed = transaction::make_transaction(
        "remove follower",
        &format!("{}/", path),
        "flake.nix",
        BuildCommand::Switch,
        |file| remove_follower_no_transaction(file, "foo"),
    )
    .unwrap();

    assert!(removed);
    let content = fs::read_to_string(format!("{}/flake.nix", path)).unwrap();
    assert!(!content.contains("follows"));
    assert!(content.contains("inputs.foo"));
}

#[test]
fn remove_input_deletes_input_block() {
    let (_dir, path) = create_flake_file(
        "{ config, lib, pkgs, ... }:\n{\n  inputs.foo = {\n    url = \"github:example/repo\";\n    follows = \"nixpkgs\";\n  };\n}\n",
    );
    let _guard = lock_build_queue();
    let removed = transaction::make_transaction(
        "remove input",
        &format!("{}/", path),
        "flake.nix",
        BuildCommand::Switch,
        |file| remove_input_no_transaction(file, "foo"),
    )
    .unwrap();

    assert!(removed);
    let content = fs::read_to_string(format!("{}/flake.nix", path)).unwrap();
    assert!(!content.contains("inputs.foo"));
}
