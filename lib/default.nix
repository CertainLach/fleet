{flake-utils}: {
  fleetConfiguration = {
    data,
    nixpkgs,
    hosts,
    ...
  } @ allConfig: let
    hostNames = nixpkgs.lib.attrNames hosts;
    config = builtins.removeAttrs allConfig ["nixpkgs" "data"];
    fleetLib = import ./fleetLib.nix {
      inherit nixpkgs hostNames;
    };
  in
    let
      withData = data: rec {
        root = nixpkgs.lib.evalModules {
          modules = (import ../modules/fleet/_modules.nix) ++ [config data];
          specialArgs = {
            inherit nixpkgs fleetLib;
          };
        };
        failedAssertions = map (x: x.message) (nixpkgs.lib.filter (x: !x.assertion) root.config.assertions);
        rootAssertWarn =
          if failedAssertions != []
          then throw "Failed assertions:\n${nixpkgs.lib.concatStringsSep "\n" (map (x: "- ${x}") failedAssertions)}"
          else nixpkgs.lib.showWarnings root.config.warnings root;
        configuredHosts = rootAssertWarn.config.hosts;
        configuredSecrets = rootAssertWarn.config.secrets;
        configuredSystems = configuredSystemsWithExtraModules [];
        configuredSystemsWithExtraModules = extraModules:
          nixpkgs.lib.listToAttrs (
            map
            (
              name: {
                inherit name;
                value = nixpkgs.lib.nixosSystem {
                  system = configuredHosts.${name}.system;
                  modules = configuredHosts.${name}.modules ++ extraModules;
                  specialArgs = {
                    inherit fleetLib;
                    fleet = fleetLib.hostsToAttrs (host: configuredSystems.${host}.config);
                  };
                };
              }
            )
            (builtins.attrNames rootAssertWarn.config.hosts)
          );
        buildSystems = {localSystem}: let
          buildConfigurationModule = {config, ...}: {
            # Equivalent to nixpkgs.localSystem
            # nixpkgs.system = localSystem;
            nixpkgs.buildPlatform.system = localSystem;
          };
        in {
          toplevel = builtins.mapAttrs (_name: value: value.config.system.build.toplevel) (configuredSystemsWithExtraModules [
            buildConfigurationModule
            ({...}: {
              buildTarget = "toplevel";
            })
          ]);
          sdImage = builtins.mapAttrs (_name: value: value.config.system.build.sdImage) (configuredSystemsWithExtraModules [
            buildConfigurationModule
            #(nixpkgs + "/nixos/modules/installer/sd-card/sd-image-aarch64-installer.nix")
            ({...}: {
              buildTarget = "sd-image";
            })
          ]);
          installationCd = builtins.mapAttrs (_name: value: value.config.system.build.isoImage) (configuredSystemsWithExtraModules [
            buildConfigurationModule
            (nixpkgs + "/nixos/modules/installer/cd-dvd/installation-cd-minimal.nix")
            ({lib, ...}: {
              buildTarget = "installation-cd";
              # Needed for https://github.com/NixOS/nixpkgs/issues/58959
              boot.supportedFilesystems = lib.mkForce ["btrfs" "reiserfs" "vfat" "f2fs" "xfs" "ntfs" "cifs"];
            })
          ]);
        };
        configUnchecked = root.config;
      };
      defaultData = withData data;
    in rec {
      inherit (defaultData) configuredHosts configuredSecrets configuredSystems buildSystems configUnchecked;
      injectData = data: let
        injectedData = withData data;
      in {
        inherit (injectedData) configuredHosts configuredSecrets configuredSystems buildSystems configUnchecked;
      };
    };
}
