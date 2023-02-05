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
        configuredHosts = rootAssertWarn.config.hosts;
        configuredSecrets = rootAssertWarn.config.secrets;
        configuredSystems = configuredSystemsWithExtraModules [ ];
        configuredSystemsWithExtraModules = extraModules: nixpkgs.lib.listToAttrs (
          map
            (
              name: {
                inherit name;
                value = nixpkgs.lib.nixosSystem {
                  system = configuredHosts.${name}.system;
                  modules = configuredHosts.${name}.modules ++ extraModules ++ [
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
        );
      in
      rec {
        inherit configuredHosts configuredSecrets configuredSystems;
        configUnchecked = root.config;
        buildSystems = {
          toplevel = builtins.mapAttrs (_name: value: value.config.system.build.toplevel) (configuredSystemsWithExtraModules [ ]);
          sdImage = builtins.mapAttrs (_name: value: value.config.system.build.sdImage) (configuredSystemsWithExtraModules [
            (nixpkgs + "/nixos/modules/installer/sd-card/sd-image-aarch64-installer.nix")
          ]);
          installationCd = builtins.mapAttrs (_name: value: value.config.system.build.isoImage) (configuredSystemsWithExtraModules [
            (nixpkgs + "/nixos/modules/installer/cd-dvd/installation-cd-minimal.nix")
            ({ lib, ... }: {
              # Needed for https://github.com/NixOS/nixpkgs/issues/58959
              boot.supportedFilesystems = lib.mkForce [ "btrfs" "reiserfs" "vfat" "f2fs" "xfs" "ntfs" "cifs" ];
            })
          ]);
        };
      });
}
