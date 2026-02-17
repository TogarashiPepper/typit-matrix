{
  description = "A port of typit to matrix";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-25.11";
    hooks = {
      url = "github:cachix/git-hooks.nix";
      inputs.nixpkgs.follows = "nixpkgs";
    };
    fenix = {
      url = "github:nix-community/fenix";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };

  outputs =
    {
      self,
      hooks,
      fenix,
      nixpkgs,
      ...
    }:
    let
      inherit (nixpkgs) lib;
      systems = [
        "aarch64-linux"
        "x86_64-linux"
      ];
      forAllSystems =
        f:
        lib.genAttrs systems (
          system:
          f rec {
            pkgs = import nixpkgs {
              inherit system;
              overlays = [ self.overlays.default ];
            };
            inherit system;
            check = self.checks.${system}.pre-commit-check;

            buildInputs = with pkgs; [
              openssl
              sqlite
            ];
            nativeBuildInputs = with pkgs; [ pkg-config ];
          }
        );
    in
    {
      overlays.default = final: prev: {
        rustToolchain =
          with fenix.packages.${prev.stdenv.hostPlatform.system};
          combine (
            (with stable; [
              clippy
              rustc
              cargo
              rust-src
              rust-analyzer
            ])
            ++ [ default.rustfmt ]
          );
      };

      checks = forAllSystems (
        {
          pkgs,
          system,
          ...
        }:
        {
          pre-commit-check = hooks.lib.${system}.run {
            src = ./.;
            hooks = {
              clippy = {
                enable = true;
                package = fenix.packages.${system}.stable.clippy;
              };
              rustfmt = {
                enable = true;
                package = fenix.packages.${system}.default.rustfmt;
              };
            };
          };
        }
      );

      packages = forAllSystems (
        {
          pkgs,
          buildInputs,
          nativeBuildInputs,
          ...
        }:
        {
          default =
            (pkgs.makeRustPlatform {
              cargo = pkgs.rustToolchain;
              rustc = pkgs.rustToolchain;
            }).buildRustPackage
              rec {
                inherit buildInputs nativeBuildInputs;

                pname = "typit-matrix";
                version = "0.1.0";
                src = ./.;
                meta.mainProgram = "typit-matrix";
                cargoLock.lockFile = ./Cargo.lock;

                OPENSSL_NO_VENDOR = 1;

                postInstall = ''
                  patchelf --set-rpath ${lib.makeLibraryPath buildInputs} $out/bin/typit-matrix
                '';
              };
        }
      );

      devShells = forAllSystems (
        {
          pkgs,
          system,
          check,
          buildInputs,
          nativeBuildInputs,
          ...
        }:
        {
          default = pkgs.mkShell {
            inherit (check) shellHook;

            packages =
              check.enabledPackages
              ++ (with pkgs; [
                rustToolchain
              ])
              ++ buildInputs
              ++ nativeBuildInputs;

            env = {
              RUST_SRC_PATH = "${pkgs.rustToolchain}/lib/rustlib/src/rust/library";
            };
          };
        }
      );
    };
}
