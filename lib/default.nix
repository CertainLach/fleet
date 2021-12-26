{ flake-utils }: {
  fleetConfiguration = { data, nixpkgs, hosts, ... }@allConfig:
    let
      config = builtins.removeAttrs allConfig [ "nixpkgs" "data" ];
      fleetLib = import ./fleetLib.nix {
        inherit nixpkgs hosts;
      };
    in
    nixpkgs.lib.genAttrs flake-utils.lib.defaultSystems (system: rec {
      root = nixpkgs.lib.evalModules {
        modules = (import ../modules/fleet/_modules.nix) ++ [ config data ];
        specialArgs = {
          inherit nixpkgs;
          fleet = fleetLib;
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
                modules = configuredHosts.${name}.modules ++ (
                  if configuredHosts.${name}.system == "aarch64-linux" then [ (nixpkgs + "/nixos/modules/installer/sd-card/sd-image-aarch64-installer.nix") ]
                  else [ ]
                ) ++ [
                  ({ ... }: {
                    nixpkgs.system = system;
                    nixpkgs.localSystem.system = system;
                    nixpkgs.crossSystem = if system == configuredHosts.${name}.system then null else {
                      system = configuredHosts.${name}.system;
                    };
                  })
                ];
                specialArgs = {
                  fleet = fleetLib.hostsToAttrs (host: configuredSystems.${host}.config);
                };
              };
            }
          )
          (builtins.attrNames root.config.hosts)
      ); #nixpkgs.lib.nixosSystem {}
    });
}
