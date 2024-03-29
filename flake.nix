{
  description = "NixOS configuration management";

  inputs = {
    nixpkgs.url = "github:nixos/nixpkgs/master";
    rust-overlay = {
      url = "github:oxalica/rust-overlay";
      inputs.nixpkgs.follows = "nixpkgs";
    };
    flake-utils = {url = "github:numtide/flake-utils";};
  };
  outputs = {
    self,
    rust-overlay,
    flake-utils,
    nixpkgs,
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
        llvmPkgs = pkgs.buildPackages.llvmPackages_11;
        rust =
          (pkgs.rustChannelOf {
            date = "2024-02-10";
            channel = "nightly";
          })
          .default
          .override {extensions = ["rust-src" "rust-analyzer"];};
      in {
        packages = (import ./pkgs) pkgs pkgs;
        devShell = (pkgs.mkShell.override {stdenv = llvmPkgs.stdenv;}) {
          nativeBuildInputs = with pkgs; [
            alejandra
            rust
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
