{
  lib,
  fleetLib,
  config,
  nixpkgs,
  ...
}: let
  inherit (fleetLib) hostsToAttrs mkFleetGeneratorDefault;
  inherit (fleetLib.types) listOfAnyModule;
  inherit (lib) mkOption mkOptionType;
  inherit (lib.types) str unspecified attrsOf listOf submodule;
  hostModule = {...} @ hostConfig: let
    hostName = hostConfig.config._module.args.name;
  in {
    options = {
      nixosModules = mkOption {
        # Not too strict, but nixos module system will fix everything.
        type =
          listOfAnyModule;

        description = "List of nixos modules";
        default = [];
      };
      system = mkOption {
        type = str;
        description = "Type of system";
      };
      encryptionKey = mkOption {
        type = str;
        description = "Encryption key";
      };
      nixosSystem = mkOption {
        type = unspecified;
        description = "Nixos configuration";
      };
      nixpkgs = mkOption {
        type = unspecified;
        description = "Nixpkgs override";
        default = nixpkgs;
      };
    };
    config = {
      nixosSystem = hostConfig.config.nixpkgs.lib.nixosSystem {
        inherit (hostConfig.config) system;
        modules = hostConfig.config.nixosModules;
        specialArgs = {
          inherit fleetLib;
          fleet = hostsToAttrs (host: config.hosts.${host}.nixosSystem.config);
        };
      };
      nixosModules.networking.hostName = mkFleetGeneratorDefault hostName;
    };
  };
  overlayType = mkOptionType {
    name = "nixpkgs-overlay";
    description = "nixpkgs overlay";
    check = lib.isFunction;
    merge = lib.mergeOneOption;
  };
in {
  options = {
    hosts = mkOption {
      type = attrsOf (submodule hostModule);
      default = {};
      description = "Configurations of individual hosts";
    };
    nixosModules = mkOption {
      type = listOfAnyModule;
      description = "Modules, which should be added to every system";
      default = [];
    };
    overlays = mkOption {
      default = [];
      type = listOf overlayType;
    };
  };
  config = {
    hosts = hostsToAttrs (host: {
      nixosModules =
        config.nixosModules
        ++ [
          {
            nixpkgs.overlays = config.overlays;
          }
        ];
    });
    nixosModules = import ../../nixos/modules/module-list.nix;
  };
}
