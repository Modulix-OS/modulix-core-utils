{
    inputs = {
        nixpkgs.url = "github:nixos/nixpkgs?ref=nixos-unstable";
        naersk.url = "github:nix-community/naersk";
    };

    outputs = { self, nixpkgs, naersk }:
    let
        supportedSystems = [
          "x86_64-linux"
          "aarch64-linux"
          "i686-linux"
        ];
        forAllSystems = f: builtins.listToAttrs (map (system: {
          name = system;
          value = f system;
        }) supportedSystems);
    in {
        packages = forAllSystems (system:
         let
            pkgs = import nixpkgs { inherit system; };
            naerskLib = pkgs.callPackage naersk {};
         in rec {
            mx-init = naerskLib.buildPackage {
                src = ./.;
                cargoBuildOptions = x: x ++ [ "--features" "init" ];
                buildInputs = with pkgs; [ openssl ];
                nativeBuildInputs = with pkgs; [ pkg-config makeWrapper ];
                postInstall = ''
                    wrapProgram $out/bin/mx-init \
                      --prefix PATH : ${pkgs.lib.makeBinPath (with pkgs; [
                        pciutils
                        usbutils
                        cpuid
                        nixos-install-tools
                        util-linux
                        nix
                      ])}
                '';
            };

            default = mx-init;
         });
        devShell = forAllSystems (system:
         let
            pkgs = import nixpkgs { inherit system; };
         in
         pkgs.mkShell {
          buildInputs = with pkgs; [
            cargo
            rust-analyzer
            rustc
            rustfmt
            openssl
            glib
            pciutils
            usbutils
            cpuid
            pkg-config
          ];
          RUST_SRC_PATH = "${pkgs.rust.packages.stable.rustPlatform.rustLibSrc}";
        });
    };
}
