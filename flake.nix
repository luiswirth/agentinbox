{
  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";

    rust-overlay.url = "github:oxalica/rust-overlay";
    rust-overlay.inputs.nixpkgs.follows = "nixpkgs";

    crane.url = "github:ipetkov/crane";

    treefmt-nix.url = "github:numtide/treefmt-nix";
    treefmt-nix.inputs.nixpkgs.follows = "nixpkgs";
  };

  outputs = {
    nixpkgs,
    rust-overlay,
    crane,
    treefmt-nix,
    ...
  }: let
    forEachSystem = f:
      nixpkgs.lib.genAttrs ["aarch64-darwin" "x86_64-linux"]
      (system:
        f (import nixpkgs {
          inherit system;
          overlays = [(import rust-overlay)];
        }));

    toolchain = pkgs: pkgs.rust-bin.fromRustupToolchainFile ./rust-toolchain.toml;
  in {
    # Every language in the tree, behind one command. The global pre-commit
    # hook checks a repository by running this, so a language left out here is
    # a language nothing checks.
    formatter = forEachSystem (pkgs:
      treefmt-nix.lib.mkWrapper pkgs {
        projectRootFile = "flake.nix";
        programs.rustfmt.enable = true;
        programs.biome.enable = true;
        programs.alejandra.enable = true;
        programs.just.enable = true;
      });

    devShells = forEachSystem (pkgs: {
      default = pkgs.mkShell {packages = [(toolchain pkgs) pkgs.just];};
    });

    packages = forEachSystem (pkgs: let
      craneLib = (crane.mkLib pkgs).overrideToolchain toolchain;
      common = {
        src = craneLib.cleanCargoSource ./.;
        pname = "agentinbox";
        version = "0";
      };
    in {
      default = craneLib.buildPackage (common // {cargoArtifacts = craneLib.buildDepsOnly common;});
    });
  };
}
