{ lib, fleet, config, ... }: with lib;
let
  host = with types; {
    options = {
      modules = mkOption {
        type = listOf (mkOptionType {
          name = "submodule";
          inherit (submodule { }) check;
          merge = lib.options.mergeOneOption;
          description = "Nixos modules";
        });
        description = "List of nixos modules";
        default = [ ];
      };
      system = mkOption {
        type = str;
        description = "Type of system";
      };
      encryptionKey = mkOption {
        type = str;
        description = "Encryption key";
      };
    };
  };
in
{
  options = with types; {
    hosts = mkOption {
      type = attrsOf (submodule host);
      default = { };
      description = "Configurations of individual hosts";
    };
    globalModules = mkOption {
      type = listOf (mkOptionType {
        name = "submodule";
        inherit (submodule { }) check;
        merge = lib.options.mergeOneOption;
        description = "Nixos modules";
      });
      description = "Modules, which should be added to every system";
      default = [ ];
    };
  };
  config = {
    hosts = fleet.hostsToAttrs (host: {
      modules = config.globalModules;
    });
    globalModules = import ../../nixos/modules/module-list.nix;
  };
}
