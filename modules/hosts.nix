{
  lib,
  fleetLib,
  config,
  ...
}: let
  inherit (fleetLib.modules) mkFleetGeneratorDefault;
  inherit (fleetLib.types) mkHostsType mkDataType;
  inherit (lib.options) mkOption;
  inherit (lib.types) str listOf attrsOf submodule;
  inherit (lib.attrsets) mapAttrsToList mapAttrs;
  inherit (lib.lists) flatten groupBy;
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
    taggedWith = mkOption {
      type = attrsOf (listOf str);
      internal = true;
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
  config.taggedWith = let
    hostTagList = flatten (mapAttrsToList (hostname: host: map (tag: {inherit hostname tag;}) host.tags) config.hosts);
    grouped = mapAttrs (_: hosts: lib.map (pair: pair.hostname) hosts) (groupBy (elem: elem.tag) hostTagList);
  in
    grouped;
  _file = ./meta.nix;
}
