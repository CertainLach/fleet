{lib, ...}: let
  inherit (lib.modules) mkRemovedOptionModule;
in {
  imports = [
    (mkRemovedOptionModule ["fleetModules"] "replaced with imports.")
    (mkRemovedOptionModule ["data"] "data is now provided by fleet itself, you can remove your import.")
  ];
}
