{ ... }: {
  nixpkgs.overlays = [ (import ../../pkgs) ];
}