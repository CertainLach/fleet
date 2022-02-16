pkgs: super:
with pkgs;
{
  fleet-install-secrets = callPackage ./fleet-install-secrets.nix { };
  fleet = callPackage ./fleet.nix { };
}
