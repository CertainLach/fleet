{
  lib,
  pkgs,
  ...
}: let
  inherit (lib) mkOption;
  inherit (lib.types) listOf str submodule;
in {
  options = {
    nixpkgs.resolvedPkgs = mkOption {
      type = lib.types.pkgs // {description = "nixpkgs.pkgs";};
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
  };
  config = {
    tags = ["all"];
    network = {};
    nixpkgs.resolvedPkgs = pkgs;
  };
}
