{ ... }: {
  nixpkgs.overlays = [ (import ../pkgs) ];
}