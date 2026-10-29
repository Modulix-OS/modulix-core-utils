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

        machines = import ./machines.nix;
        mkFakehw = pkgs:
        let
            inherit (pkgs.lib) concatMapStrings concatMapStringsSep optionalString mapAttrsToList;
            inherit (pkgs.lib.strings) concatImapStrings escapeShellArg;

            mkMachine = name: m: pkgs.runCommand "fakehw-${name}" { } ''
                sys=$out/sys
                dmi=$sys/devices/virtual/dmi/id
                mkdir -p "$dmi" "$sys/block" "$sys/bus/usb/devices"
                printf '%s\n' ${escapeShellArg m.vendor}  > "$dmi/sys_vendor"
                ${optionalString (m.family != null) ''printf '%s\n' ${escapeShellArg m.family} > "$dmi/product_family"''}
                printf '%s\n' ${escapeShellArg m.product} > "$dmi/product_name"
                ${concatMapStrings (d: ''
                  mkdir -p "$sys/block/${d.name}/queue"
                  echo ${if d.ssd then "0" else "1"} > "$sys/block/${d.name}/queue/rotational"
                '') m.disks}
                ${optionalString m.battery ''mkdir -p "$sys/class/power_supply/BAT0"''}
                ${optionalString m.iio ''mkdir -p "$sys/bus/iio/devices/iio:device0"''}
                ${concatImapStrings (i: u: ''
                  mkdir -p "$sys/bus/usb/devices/1-${toString i}"
                  echo ${u.id} > "$sys/bus/usb/devices/1-${toString i}/idVendor"
                  printf '%s\n' ${escapeShellArg u.product} > "$sys/bus/usb/devices/1-${toString i}/product"
                '') m.usb}
                printf '%s\n' ${concatMapStringsSep " " escapeShellArg m.lspci} > $out/lspci.txt
                printf '%s\n' ${escapeShellArg m.cpuid} > $out/cpuid.txt
            '';

            farm = pkgs.linkFarm "fakehw-machines"
              (mapAttrsToList (name: m: { inherit name; path = mkMachine name m; }) machines);

            lspciStub = pkgs.writeShellScriptBin "lspci" ''exec cat "$FAKE_HW/lspci.txt"'';
            cpuidStub = pkgs.writeShellScriptBin "cpuid" ''exec cat "$FAKE_HW/cpuid.txt"'';
        in pkgs.writeShellScriptBin "fakehw" ''
            set -euo pipefail
            if [ $# -eq 0 ] || [ "$1" = "--list" ]; then
              echo "usage: fakehw <machine> [command...]"
              echo "available machines:"
              ls ${farm} | sed 's/^/  /'
              exit 0
            fi
            machine="$1"; shift
            [ -e "${farm}/$machine" ] || { echo "unknown machine: $machine" >&2; exit 1; }
            FAKE_HW="$(realpath "${farm}/$machine")"
            export FAKE_HW
            export PATH="${lspciStub}/bin:${cpuidStub}/bin:$PATH"
            exec ${pkgs.bubblewrap}/bin/bwrap \
              --dev-bind / / \
              --ro-bind "$FAKE_HW/sys" /sys \
              --setenv FAKE_HW "$FAKE_HW" \
              -- "''${@:-$SHELL}"
        '';
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

            fakehw = mkFakehw pkgs; # fakehw

            default = mx-init;
         });

        apps = forAllSystems (system: { # fakehw
          fakehw = {
            type = "app";
            program = "${self.packages.${system}.fakehw}/bin/fakehw";
          };
        });

        devShells = forAllSystems (system:
         let
            pkgs = import nixpkgs { inherit system; };
         in {
          default = pkgs.mkShell {
            buildInputs = with pkgs; [
              cargo
              rust-analyzer
              rustc
              clippy # fakehw
              rustfmt
              openssl
              glib
              pciutils
              usbutils
              cpuid
              pkg-config
              (mkFakehw pkgs) # fakehw
            ];
            RUST_SRC_PATH = "${pkgs.rust.packages.stable.rustPlatform.rustLibSrc}";
          };
        });
    };
}
