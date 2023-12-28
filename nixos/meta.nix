{
  lib,
  pkgs,
  ...
}:
with lib; {
  options = with types; {
    nixpkgs.resolvedPkgs = mkOption {
      type = types.pkgs // {description = "nixpkgs.pkgs";};
      description = "Value of pkgs";
    };
    tags = mkOption {
      type = listOf str;
      description = "Host tags";
      default = [];
    };
    network = mkOption {
      type = submodule {
        options = {
          internalIps = mkOption {
            type = listOf str;
            description = "Internal ips";
            default = [];
          };
          externalIps = mkOption {
            type = listOf str;
            description = "External ips";
            default = [];
          };
        };
      };
      description = "Network definition of host";
    };
    buildTarget = mkOption {
      type = enum ["toplevel" "sd-image" "installation-cd"];
    };
  };
  config = {
    tags = ["all"];
    network = {};
    nixpkgs.resolvedPkgs = pkgs;
  };
}
