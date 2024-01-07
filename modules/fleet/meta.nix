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
        modules = mkOption {
          type = listOf (mkOptionType {
            name = "submodule";
            inherit (submodule {}) check;
            merge = lib.options.mergeOneOption;
            description = "Nixos modules";
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
      };
      config = {
        nixosSystem = nixpkgs.lib.nixosSystem {
          inherit (hostConfig.config) system modules;
          specialArgs = {
            inherit fleetLib;
            fleet = hostsToAttrs (host: config.hosts.${host}.nixosSystem.config);
          };
        };
        modules = [
          ({...}: {
            networking.hostName = mkFleetDefault hostName;
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
    globalModules = mkOption {
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
      modules =
        config.globalModules
        ++ [
          ({...}: {
            nixpkgs.overlays = config.overlays;
          })
        ];
    });
    globalModules = import ../../nixos/modules/module-list.nix;
  };
}
