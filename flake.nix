{
  description = "Project generated from rust template";

  inputs = {
    nixpkgs.url = "nixpkgs/nixos-unstable";
    flake-utils.url = "github:numtide/flake-utils";
    naersk = {
      url = "github:nix-community/naersk";
      inputs.nixpkgs.follows = "nixpkgs";
    };
    fenix = {
      url = "github:nix-community/fenix";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };

  outputs = {
    self,
    nixpkgs,
    naersk,
    flake-utils,
    fenix,
  } @ inputs:
    flake-utils.lib.eachDefaultSystem (
      system: let
        pkgs = nixpkgs.legacyPackages.${system};
        fPkgs = fenix.packages.${system};
        rustPkg =
          if builtins.pathExists ./rust-toolchain.toml
          then
            fPkgs.fromToolchainFile {
              file = ./rust-toolchain.toml;
              # replace with pkgs.lib.fakeSha256 on edit
              sha256 = pkgs.lib.fakeSha256;
            }
          else if builtins.pathExists ./rust-toolchain
          then
            fPkgs.fromToolchainFile {
              file = ./rust-toolchain;
              # replace with pkgs.lib.fakeSha256 on edit
              sha256 = pkgs.lib.fakeSha256;
            }
          else
            fPkgs.combine [
              fPkgs.stable.cargo
              fPkgs.stable.rustc
              fPkgs.stable.rust-std
              fPkgs.stable.rust-src
              fPkgs.rust-analyzer
              fPkgs.stable.clippy-preview
              fPkgs.stable.rustfmt
            ];
        naersk' = pkgs.callPackage naersk {
          cargo = rustPkg;
          rustc = rustPkg;
          clippy = rustPkg;
        };
        rust-project = naersk'.buildPackage {
          src = ./.;
          targets = [system];
          buildInputs = with pkgs; [
          ];
        };
      in rec {
        formatter = pkgs.alejandra;

        packages.default = rust-project;
        devShells.default = pkgs.mkShell {
          buildInputs = with pkgs; [];

          nativeBuildInputs = with pkgs; [
            rustPkg
            bacon
            watchexec
          ];
          shellHook = ''
            echo "host-arch: ${system}"
          '';
        };
      }
    );
}
