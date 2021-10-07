{
  fleetConfiguration = { data, nixpkgs, hosts, ... }@allConfig:
    let
      config = builtins.removeAttrs allConfig [ "nixpkgs" "data" ];
    in
    rec {
      root = nixpkgs.lib.evalModules {
        modules = (import ../modules/fleet/_modules.nix) ++ [ config data ];
        specialArgs = {
          inherit nixpkgs;
          fleet = import ./fleetLib.nix {
            inherit nixpkgs hosts;
          };
        };
      };
      configuredHosts = root.config.hosts;
      configuredSecrets = root.config.secrets;
      configuredSystems = nixpkgs.lib.listToAttrs (
        map
          (
            name: {
              inherit name;
              value = nixpkgs.lib.nixosSystem {
                system = configuredHosts.${name}.system;
                modules = configuredHosts.${name}.modules;
                pkgs = import nixpkgs { system = configuredHosts.${name}.system; };
              };
            }
          )
          (builtins.attrNames root.config.hosts)
      ); #nixpkgs.lib.nixosSystem {}
    };
}
