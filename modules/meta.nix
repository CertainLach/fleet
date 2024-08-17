{lib, ...}: let
  inherit (lib.modules) mkRemovedOptionModule;
in {
  imports = [
    (mkRemovedOptionModule ["fleetModules"] "replaced with imports.")
  ];
}
