{
  callPackage,
  craneLib,
}: rec {
  default = fleet;

  fleet-install-secrets = callPackage ./fleet-install-secrets.nix {inherit craneLib;};
  fleet = callPackage ./fleet.nix {inherit craneLib;};
}
