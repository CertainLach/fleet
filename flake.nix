{
  description = "NixOS configuration management";

  inputs = {
    nixpkgs.url = "github:nixos/nixpkgs";
  };
  outputs = { self, nixpkgs }: with nixpkgs.lib; rec {
    lib = import ./lib;
  };
}
