{
  lib,
  fleetLib,
  inputs,
  config,
  ...
}: let
  inherit (lib.attrsets) mapAttrs;
  inherit (lib.options) mkOption;
  inherit (lib.types) deferredModule;
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
          type = deferredModule;
          apply = module:
            config.nixpkgs.buildUsing.lib.nixosSystem {
              inherit (hostArgs.config) system;
              modules = [
                (module // {key = "attr<host.nixos>";})
                (config.nixos // {key = "attr<fleet.nixos>";})
              ];
              specialArgs = {
                inherit fleetLib inputs;
              };
            };
        };
      };
      config = {
        # imports = [
        #   (mkRemovedOptionModule ["nixosModules"] "replaced with hosts.*.nixos.imports.")
        # ];
        nixos = {
          config._module.args = {
            nixosHosts = mapAttrs (_: value: value.nixos.config) config.hosts;
            hosts = config.hosts;
            host = hostArgs.config;
          };
        };
      };
    });
  };
  imports = [
    (mkRemovedOptionModule ["nixosModules"] "replaced with nixos.imports.")
  ];
  config.nixos.imports =
    import ./nixos/module-list.nix;
}
