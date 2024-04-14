{
  description = "NixOS configuration management";

  inputs = {
    nixpkgs.url = "github:nixos/nixpkgs/master";
    rust-overlay = {
      url = "github:oxalica/rust-overlay";
      inputs = {
        nixpkgs.follows = "nixpkgs";
        flake-utils.follows = "flake-utils";
      };
    };
    flake-utils.url = "github:numtide/flake-utils";
    crane = {
      url = "github:ipetkov/crane";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };
  outputs = {
    self,
    rust-overlay,
    flake-utils,
    nixpkgs,
    crane,
  }:
    with nixpkgs.lib;
      {
        lib = import ./lib {inherit flake-utils;};
      }
      // flake-utils.lib.eachDefaultSystem (system: let
        pkgs =
          import nixpkgs
          {
            inherit system;
            overlays = [(import rust-overlay)];
          };
        rust = pkgs.rust-bin.fromRustupToolchainFile ./rust-toolchain.toml;
        craneLib = (crane.mkLib pkgs).overrideToolchain rust;
      in {
        packages = import ./pkgs {
          inherit (pkgs) callPackage;
          inherit craneLib;
        };
        devShell = craneLib.devShell {
          nativeBuildInputs = with pkgs; [
            alejandra
            lld
            cargo-edit
            cargo-udeps
            cargo-fuzz
            cargo-watch
            cargo-outdated

            pkg-config
            openssl
            bacon
          ];
        };
      });
}
