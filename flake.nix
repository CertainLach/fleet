{
  description = "NixOS configuration management";

  inputs = {
    nixpkgs.url = "github:nixos/nixpkgs/master";
    nixpkgs-stable-for-tests.url = "github:nixos/nixpkgs/nixos-23.11";
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
    nixpkgs-stable-for-tests,
    crane,
  }:
    with nixpkgs.lib;
      {
        lib = import ./lib {
          inherit flake-utils;
          fleetPkgsForPkgs = pkgs: import ./pkgs {
            inherit (pkgs) callPackage;
            craneLib = crane.mkLib pkgs;
          };
        };
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
        packages = let
          packages = import ./pkgs {
            inherit (pkgs) callPackage;
            inherit craneLib;
          };
        in
          packages // {default = packages.fleet;};

        checks = let
          packages = import ./pkgs {
            inherit (pkgs) callPackage;
            craneLib = crane.mkLib (import nixpkgs {inherit system;});
          };
          packages-with-nixpkgs-stable = import ./pkgs {
            inherit (pkgs) callPackage;
            craneLib = crane.mkLib (import nixpkgs-stable-for-tests {inherit system;});
          };
          prefixAttrs = prefix: attrs:
            nixpkgs.lib.attrsets.mapAttrs' (name: value: {
              name = "${prefix}${name}";
              value = value.overrideAttrs (prev: {
                pname = "${prefix}${prev.pname}";
              });
            })
            attrs;
        in
          # `fleet` crate wants nightly rust, also little sense of supporting it on stable nixpkgs.
          (prefixAttrs "nixpkgs-" (removeAttrs packages ["fleet"]))
          // (prefixAttrs "nixpkgs-stable-" (removeAttrs packages-with-nixpkgs-stable ["fleet"]));

        devShells.default = craneLib.devShell {
          packages = with pkgs; [
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
