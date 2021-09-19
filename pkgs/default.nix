pkgs: super:
with pkgs;
{
  fleet-install-secrets = callPackage ./fleet-install-secrets.nix { };
}
