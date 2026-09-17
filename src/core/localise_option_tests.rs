/// Tests for the AST walks of `localise_option`.
///
/// Pure parsing — no file, no transaction.
use super::{collect_child_names, collect_enable_paths};

fn enable_paths(src: &str, prefix: &str) -> Vec<String> {
    let ast = rnix::Root::parse(src);
    collect_enable_paths(&ast.syntax(), prefix)
}

/// The spelling `mx-daemon` writes: one nested set per path segment.
#[test]
fn nested_module_path_is_returned_whole() {
    let src = r#"{config, lib, pkgs, ...}:
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
"#;
    assert_eq!(enable_paths(src, "mx"), vec!["programs.studio.obs-studio"]);
}

/// The same module, hand-written flat. Must yield the identical name.
#[test]
fn flat_module_path_is_returned_whole() {
    let src = "{\n  mx.programs.studio.obs-studio.enable = true;\n}\n";
    assert_eq!(enable_paths(src, "mx"), vec!["programs.studio.obs-studio"]);
}

/// Mixed spellings in one file, and a single-segment module alongside.
#[test]
fn mixed_spellings_and_depths_are_merged() {
    let src = r#"{
  mx = {
    fonts.enable = true;
    programs = {
      studio.obs-studio.enable = false;
    };
  };
  mx.programs.games.umu = {
    enable = true;
  };
}
"#;
    // The value is not read here — `enable = false` still declares a module.
    assert_eq!(
        enable_paths(src, "mx"),
        vec!["fonts", "programs.games.umu", "programs.studio.obs-studio"]
    );
}

/// Only `enable` counts: a module carrying just a plugin list is not a
/// candidate, and `mx.enable` itself (no module segment) is never one.
#[test]
fn non_enable_attributes_are_ignored() {
    let src = r#"{
  mx = {
    enable = true;
    programs.studio.obs-studio.plugins = [ pkgs.obs-studio-plugins.wlrobs ];
  };
}
"#;
    assert!(enable_paths(src, "mx").is_empty());
}

/// Attributes outside the prefix never leak in.
#[test]
fn other_prefixes_are_ignored() {
    let src = "{\n  services.openssh.enable = true;\n  mx.fonts.enable = true;\n}\n";
    assert_eq!(enable_paths(src, "mx"), vec!["fonts"]);
}

/// Why `collect_child_names` cannot be used to list modules: it stops at the
/// first segment, which is a namespace, not a module name.
#[test]
fn child_names_only_sees_the_first_segment() {
    let src = "{\n  mx.programs.studio.obs-studio.enable = true;\n}\n";
    let ast = rnix::Root::parse(src);
    assert_eq!(collect_child_names(&ast.syntax(), "mx"), vec!["programs"]);
}
