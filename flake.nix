{
  description = "NixOS configuration management";

  inputs = {
    nixpkgs.url = "github:nixos/nixpkgs/staging-next";
    rust-overlay = { url = "github:oxalica/rust-overlay"; flake = false; };
    flake-utils.url = "github:numtide/flake-utils";
  };
  outputs = { self, rust-overlay, flake-utils, nixpkgs }: with nixpkgs.lib; rec {
    lib = import ./lib;
  } // flake-utils.lib.eachDefaultSystem (system:
    let
      pkgs = import nixpkgs
        {
          inherit system; overlays = [ (import rust-overlay) ];
        };
      llvmPkgs = pkgs.buildPackages.llvmPackages_11;
      rust = (pkgs.rustChannelOf { date = "2021-08-16"; channel = "nightly"; }).default.override { extensions = [ "rust-src" ]; };
      rustPlatform = pkgs.makeRustPlatform { cargo = rust; rustc = rust; };
    in
    {
      devShell = (pkgs.mkShell.override { stdenv = llvmPkgs.stdenv; }) {
        nativeBuildInputs = with pkgs; [
          rust
          cargo-edit
          cargo-udeps
          cargo-fuzz

          pkgconfig
          openssl
        ];
      };
    });
}
