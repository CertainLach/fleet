{ flake-utils }: {
  fleetConfiguration = { data, nixpkgs, hosts, ... }@allConfig:
    let
      hostNames = nixpkgs.lib.attrNames hosts;
      config = builtins.removeAttrs allConfig [ "nixpkgs" "data" ];
      fleetLib = import ./fleetLib.nix {
        inherit nixpkgs hostNames;
      };
    in
    nixpkgs.lib.genAttrs flake-utils.lib.defaultSystems (system:
      let
        root = nixpkgs.lib.evalModules {
          modules = (import ../modules/fleet/_modules.nix) ++ [ config data ];
          specialArgs = {
            inherit nixpkgs fleetLib;
          };
        };
        failedAssertions = map (x: x.message) (nixpkgs.lib.filter (x: !x.assertion) root.config.assertions);
        rootAssertWarn =
          if failedAssertions != [ ]
          then throw "Failed assertions:\n${nixpkgs.lib.concatStringsSep "\n" (map (x: "- ${x}") failedAssertions)}"
          else nixpkgs.lib.showWarnings root.config.warnings root;
      in
      rec {
        configuredHosts = rootAssertWarn.config.hosts;
        configuredSecrets = rootAssertWarn.config.secrets;
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
                    inherit fleetLib;
                    fleet = fleetLib.hostsToAttrs (host: configuredSystems.${host}.config);
                  };
                };
              }
            )
            (builtins.attrNames rootAssertWarn.config.hosts)
        ); #nixpkgs.lib.nixosSystem {}
      });
}
