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
}
