{
  lib,
  pkgs,
  ...
}: let
  inherit (lib.options) mkOption;
  inherit (lib.types) listOf str submodule;
  inherit (lib.modules) mkRemovedOptionModule;
in {
  options = {
    # TODO: Give a real name.
    # Previously it was nixpkgs.resolvedPkgs, which was erroreously merged with nixpkgs override attribute.
    _resolvedPkgs = mkOption {
      type = lib.types.pkgs // {description = "nixpkgs.pkgs";};
      description = "Value of pkgs";
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
  imports = [
    (mkRemovedOptionModule ["tags"] "tags are now defined at the host level, not the nixos system level for fast filtering without evaluating unnecessary hosts.")
  ];
  config = {
    network = {};
    _resolvedPkgs = pkgs;
  };
}
