{
  lib,
  pkgs,
  ...
}: let
  inherit (lib.options) mkOption;
  inherit (lib.modules) mkRemovedOptionModule;
in {
  options = {
    # TODO: Give a real name.
    # Previously it was nixpkgs.resolvedPkgs, which was erroreously merged with nixpkgs override attribute.
    _resolvedPkgs = mkOption {
      type = lib.types.pkgs // {description = "nixpkgs.pkgs";};
      description = "Value of pkgs";
    };
  };
  imports = [
    (mkRemovedOptionModule ["tags"] "tags are now defined at the host level, not the nixos system level for fast filtering without evaluating unnecessary hosts.")
    (mkRemovedOptionModule ["network"] "network is now defined at the host level, not the nixos system level")
  ];
  config = {
    _resolvedPkgs = pkgs;
  };
}
