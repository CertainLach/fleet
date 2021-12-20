{ lib, ... }:
with lib;
{
  options = with types; {
    tags = mkOption {
      type = listOf str;
      description = "Host tags";
      default = [ ];
    };
    network = mkOption {
      type = submodule {
        options = {
          internalIps = mkOption {
            type = listOf str;
            description = "Internal ips";
            default = [ ];
          };
          externalIps = mkOption {
            type = listOf str;
            description = "External ips";
            default = [ ];
          };
        };
      };
      description = "Network definition of host";
    };
  };
  config = {
    tags = [ "all" ];
    network = { };
  };
}
