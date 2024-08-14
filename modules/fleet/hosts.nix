{
  lib,
  fleetLib,
  ...
}: let
  inherit (fleetLib.modules) mkFleetGeneratorDefault;
  inherit (fleetLib.types) mkHostsType;
  inherit (lib.options) mkOption;
  inherit (lib.types) str listOf;
in {
  options = {
    hosts = mkOption {
      type = mkHostsType ({config, ...}: {
        options = {
          system = mkOption {
            type = str;
            description = "Type of the system.";
          };
          # TODO: This is part of fleet.nix, move it to separate toplevel data config option.
          encryptionKey = mkOption {
            type = str;
            description = "Rage SSH encryption key for secrets.";
          };
          tags = mkOption {
            type = listOf str;
            description = "Host tag. In CLI, you can refer to all hosts having this tag using @tag syntax.";
          };
        };
        config = {
          nixos.networking.hostName = mkFleetGeneratorDefault config._module.args.name;
          tags = ["all"];
        };
        _file = ./meta.nix;
      });
      default = {};
      description = "Configurations of individual hosts";
    };
  };
  _file = ./meta.nix;
}
