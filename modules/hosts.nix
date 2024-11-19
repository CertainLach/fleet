{
  lib,
  fleetLib,
  ...
}: let
  inherit (fleetLib.modules) mkFleetGeneratorDefault;
  inherit (fleetLib.types) mkHostsType mkDataType;
  inherit (lib.options) mkOption;
  inherit (lib.types) str listOf attrsOf submodule;
in {
  options = {
    data = mkOption {
      type = mkDataType {
        options = {
          version = mkOption {
            type = str;
            internal = true;
          };
          gcRootPrefix = mkOption {
            type = str;
            internal = true;
          };
          hosts = mkOption {
            type = attrsOf (submodule {
              options.encryptionKey = mkOption {
                type = str;
                description = "Rage SSH encryption key for secrets.";
              };
            });
          };
        };
      };
      description = ''
        Configuration provided from outside.
        Usually used to persist fleet data between runs.
      '';
    };
    hosts = mkOption {
      type = mkHostsType ({config, ...}: {
        options = {
          system = mkOption {
            description = "Type of the system.";
            type = str;
            example = "x86_64-linux";
          };
          tags = mkOption {
            description = "Host tag. In CLI, you can refer to all hosts having this tag using @tag syntax.";
            type = listOf str;
          };
          network = mkOption {
            type = submodule {
              options = {
                internalIps = mkOption {
                  description = "Internal ips";
                  type = listOf str;
                  default = [];
                };
                externalIps = mkOption {
                  description = "External ips";
                  type = listOf str;
                  default = [];
                };
              };
            };
            description = "Network definition of host";
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
