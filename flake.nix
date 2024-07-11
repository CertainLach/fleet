{
  description = "NixOS configuration management";

  inputs = {
    nixpkgs.url = "github:nixos/nixpkgs/master";
    nixpkgs-stable-for-tests.url = "github:nixos/nixpkgs/nixos-24.05";
    rust-overlay = {
      url = "github:oxalica/rust-overlay";
      inputs = {
        nixpkgs.follows = "nixpkgs";
      };
    };
    flake-parts.url = "github:hercules-ci/flake-parts";
    crane = {
      url = "github:ipetkov/crane";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };
  outputs = inputs @ {
    self,
    flake-parts,
    crane,
    ...
  }:
    flake-parts.lib.mkFlake {
      inherit inputs;
    } {
      flake = let
        inherit (inputs.nixpkgs.lib) mapAttrs;
      in {
        lib = import ./lib {
          fleetPkgsForPkgs = pkgs:
            import ./pkgs {
              inherit (pkgs) callPackage;
              craneLib = crane.mkLib pkgs;
            };
        };
        # To be used with https://github.com/NixOS/nix/pull/8892
        schemas = {
          fleetConfigurations = {
            version = 1;
            doc = ''
              The `fleetConfigurations` flake output defines fleet cluster configurations.
            '';
            inventory = output: {
              children =
                mapAttrs (configName: cluster: {
                  what = "fleet cluster configuration";

                  children =
                    mapAttrs (hostName: host: {
                      what = "host [${host.system}]";
                    })
                    cluster.config.hosts;
                  # It is possible to implement this inventory right now, but I want to
                  # get rid of `fleet.nix` file in the future.
                  # children.secrets = { };
                })
                output;
            };
          };
        };
      };
      # Supported and tested list of deployment targets.
      systems = ["x86_64-linux" "aarch64-linux" "armv7l-linux" "armv6l-linux"];
      perSystem = {
        config,
        system,
        pkgs,
        ...
      }: let
        inherit (lib) mapAttrs' elem;
        # Can also be built for darwin, through it is not usual to deploy nixos systems from macos machines.
        # I have no hardware for such testing, thus only adding machines I actually have and use.
        #
        # It is not possible to deploy any host from armv6/armv7 hardware, and I don't think it even makes sense.
        deployerSystems = ["aarch64-linux" "x86_64-linux"];
        deployerSystem = elem system deployerSystems;
        lib = pkgs.lib;
        rust = pkgs.rust-bin.fromRustupToolchainFile ./rust-toolchain.toml;
        craneLib = (crane.mkLib pkgs).overrideToolchain rust;
      in {
        _module.args.pkgs = import inputs.nixpkgs {
          inherit system;
          overlays = [(inputs.rust-overlay.overlays.default)];
        };
        # Reference fleet package should be built with nightly rust, specified in rust-toolchain.toml.
        packages = lib.mkIf deployerSystem (let
          packages = import ./pkgs {
            inherit (pkgs) callPackage;
            inherit craneLib;
          };
        in
          packages // {default = packages.fleet;});
        # TODO: It should be possible to move lib.mkIf to default attribute, instead of disabling the whole
        # devShells block, yet nix flake check fails here, due to no default shell found. It is nix or flake-parts bug?
        devShells = lib.mkIf deployerSystem {
          default = craneLib.devShell {
            packages = with pkgs; [
              rust
              alejandra
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
        };
        # fleet-install-secrets will not be built normally, because they are not ran directly by user most of the time.
        # checks there build packages for default nixpkgs rustPlatform packages.
        checks = let
          packages = import ./pkgs {
            inherit (pkgs) callPackage;
            craneLib = crane.mkLib pkgs;
          };
          packages-with-nixpkgs-stable = import ./pkgs {
            inherit (pkgs) callPackage;
            craneLib = crane.mkLib (import inputs.nixpkgs-stable-for-tests {inherit system;});
          };
          prefixAttrs = prefix: attrs:
            mapAttrs' (name: value: {
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
        formatter = pkgs.alejandra;
      };
    };
}
