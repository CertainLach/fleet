{
  description = "NixOS configuration management";

  inputs = {
    nixpkgs.url = "github:nixos/nixpkgs/master";
    rust-overlay = { url = "github:oxalica/rust-overlay"; inputs.nixpkgs.follows = "nixpkgs"; };
    flake-utils = { url = "github:numtide/flake-utils"; };
  };
  outputs = { self, rust-overlay, flake-utils, nixpkgs }: with nixpkgs.lib; rec {
    lib = import ./lib { inherit flake-utils; };
  } // flake-utils.lib.eachDefaultSystem (system:
    let
      pkgs = import nixpkgs
        {
          inherit system; overlays = [ (import rust-overlay) ];
        };
      llvmPkgs = pkgs.buildPackages.llvmPackages_11;
      rust = (pkgs.rustChannelOf { date = "2023-10-20"; channel = "nightly"; }).default.override { extensions = [ "rust-src" "rust-analyzer" ]; };
      rustPlatform = pkgs.makeRustPlatform { cargo = rust; rustc = rust; };
    in
    {
      devShell = (pkgs.mkShell.override { stdenv = llvmPkgs.stdenv; }) {
        nativeBuildInputs = with pkgs; [
          rust
          lld
          cargo-edit
          cargo-udeps
          cargo-fuzz

          pkg-config
          openssl
          bacon
        ];
      };
    });
}
