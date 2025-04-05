{ lib, ... }:
let
  inherit (lib.modules) mkRemovedOptionModule;
in
{
  imports = [
    (mkRemovedOptionModule [ "tags" ]
      "tags are now defined at the host level, not the nixos system level for fast filtering without evaluating unnecessary hosts."
    )
    (mkRemovedOptionModule [
      "network"
    ] "network is now defined at the host level, not the nixos system level")
  ];

  # Version of environment (fleet scripts such as rollback) already installed on the host
  config.environment.etc.FLEET_HOST.text = "1";
}
