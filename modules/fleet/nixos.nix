{
  lib,
  fleetLib,
  config,
  ...
}: let
  inherit (lib.attrsets) mapAttrs;
  inherit (lib.options) mkOption;
  inherit (lib.types) deferredModule deferredModuleWith;
  inherit (lib.modules) mkRemovedOptionModule;
  inherit (fleetLib.options) mkHostsOption;

  _file = ./nixos.nix;
in {
  options = {
    nixos = mkOption {
      description = ''
        Nixos configuration for all hosts.
      '';
      type = deferredModule;
    };
    hosts = mkHostsOption (hostArgs: {
      inherit _file;
      options = {
        nixos = mkOption {
          description = ''
            Nixos configuration for the current host.
          '';
          type = deferredModuleWith {
            staticModules = import ../../nixos/modules/module-list.nix;
          };
          apply = module:
            config.nixpkgs.buildUsing.lib.nixosSystem {
              inherit (hostArgs.config) system;
              modules = [module];
            };
        };
      };
      config = {
        # imports = [
        #   (mkRemovedOptionModule ["nixosModules"] "replaced with hosts.*.nixos.imports.")
        # ];
        nixos = {
          imports = [
            config.nixos
          ];
          config._module.args.fleet = mapAttrs (_: value: value.nixos.config) config.hosts;
        };
      };
    });
  };
  imports = [
    (mkRemovedOptionModule ["nixosModules"] "replaced with nixos.imports.")
  ];
}
