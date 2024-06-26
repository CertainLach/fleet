{
  lib,
  fleetLib,
  config,
  nixpkgs,
  ...
}:
with lib;
with fleetLib; let
  hostModule = with types;
    {...} @ hostConfig: let
      hostName = hostConfig.config._module.args.name;
    in {
      options = {
        nixosModules = mkOption {
          type = listOf (mkOptionType {
            name = "submodule";
            inherit (submodule {}) check;
            merge = lib.options.mergeOneOption;
            description = "Nixos module";
          });
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
        nixosModules = [
          ({...}: {
            networking.hostName = mkFleetGeneratorDefault hostName;
          })
        ];
      };
    };
  overlayType = mkOptionType {
    name = "nixpkgs-overlay";
    description = "nixpkgs overlay";
    check = lib.isFunction;
    merge = lib.mergeOneOption;
  };
in {
  options = with types; {
    hosts = mkOption {
      type = attrsOf (submodule hostModule);
      default = {};
      description = "Configurations of individual hosts";
    };
    nixosModules = mkOption {
      type = listOf (mkOptionType {
        name = "submodule";
        inherit (submodule {}) check;
        merge = lib.options.mergeOneOption;
        description = "Nixos modules";
      });
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
          ({...}: {
            nixpkgs.overlays = config.overlays;
          })
        ];
    });
    nixosModules = import ../../nixos/modules/module-list.nix;
  };
}
