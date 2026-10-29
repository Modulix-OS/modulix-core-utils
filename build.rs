use std::path::{Path, PathBuf};

/// Stub table used when `flathub_basic_info.rs` is absent, so the library still
/// compiles and the `flathub-info-gen` binary - which is what produces that
/// file - can be built from a checkout that does not carry it yet.
const EMPTY_TABLE: &str = "\
static NIX_INFO: phf::Map<&'static str, NixInfo> = phf::phf_map! {};\n\n\
static APP_ID_TO_PACKAGES: phf::Map<&'static str, &'static [&'static str]> = phf::phf_map! {};\n";

fn main() {
    let arch = std::env::consts::ARCH;
    let os = std::env::consts::OS;

    let target_nix = match (arch, os) {
        ("x86_64", "linux") => "x86_64-linux",
        ("aarch64", "linux") => "aarch64-linux",
        ("x86", "linux") => "i686-linux",
        _ => "unknown",
    };

    // Inject the value as a compile-time environment variable
    println!("cargo:rustc-env=TARGET_NIX={}", target_nix);

    // Stage the generated Flathub lookup table in OUT_DIR, falling back to an
    // empty one, so `package_basic_info.rs` can include it unconditionally.
    let src = Path::new("flathub_basic_info.rs");
    let dest = PathBuf::from(std::env::var_os("OUT_DIR").expect("OUT_DIR is set by cargo"))
        .join("flathub_basic_info.rs");
    println!("cargo:rerun-if-changed=flathub_basic_info.rs");
    if src.exists() {
        std::fs::copy(src, &dest).expect("Impossible to stage flathub_basic_info.rs");
    } else {
        std::fs::write(&dest, EMPTY_TABLE).expect("Impossible to write the empty lookup table");
    }
}
