{ lib, fleet, ... }: with lib;
let
  host = with types; {
    options = {
      modules = mkOption {
        type = listOf anything;
        description = "List of nixos modules";
        default = [ ];
      };
      network = mkOption {
        type = submodule {
          options = {
            fleetIp = {
              type = str;
              description = "Ip which is available to all hosts in fleet";
            };
          };
        };
        description = "Network definition of host";
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
  };
  config.hosts = fleet.hostsToAttrs (host: {
    modules = [
      ({ ... }: {
        nixpkgs.overlays = [ (import ../pkgs) ];
      })
    ];
  });
}
