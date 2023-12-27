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
  in let
    root = nixpkgs.lib.evalModules {
      modules = (import ../modules/fleet/_modules.nix) ++ [config data];
      specialArgs = {
        inherit nixpkgs fleetLib;
      };
    };
    failedAssertions = map (x: x.message) (nixpkgs.lib.filter (x: !x.assertion) root.config.assertions);
    checkedRoot =
      if failedAssertions != []
      then throw "Fleet failed assertions:\n${nixpkgs.lib.concatStringsSep "\n" (map (x: "- ${x}") failedAssertions)}"
      else nixpkgs.lib.showWarnings root.config.warnings root;
    withData = {
      root,
      data,
    }: rec {
      configuredHosts = root.config.hosts;
      configuredUncheckedHosts = root.config.hosts;
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
          (builtins.attrNames root.config.hosts)
        );
      buildableSystems = {localSystem}: let
        buildConfigurationModule = {config, ...}: {
          # Equivalent to nixpkgs.localSystem
          # nixpkgs.system = localSystem;
          nixpkgs.buildPlatform.system = localSystem;
        };
      in
        configuredSystemsWithExtraModules [
          buildConfigurationModule
        ];
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
    defaultData = withData {
      inherit data;
      root = checkedRoot;
    };
    uncheckedData = withData {inherit data root;};
  in rec {
    inherit (defaultData) configuredHosts configuredSystems buildSystems configUnchecked buildableSystems;
    unchecked = {
      inherit (uncheckedData) configuredHosts configuredSystems buildSystems configUnchecked buildableSystems;
    };
    injectData = data: let
      injectedData = withData data;
    in {
      inherit (injectedData) configuredHosts configuredSystems buildSystems configUnchecked;
    };
  };
}
