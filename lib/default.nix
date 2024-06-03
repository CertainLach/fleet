{fleetPkgsForPkgs}: {
  fleetConfiguration = {
    # TODO: Provide by fleet, instead of requesting user to provide it.
    # This is not good that user needs to provide it, as it becomes a flake data, and fleet arbitrarily rewriting it
    # always dirnets the flake. Instead, fleetConfiguration should return function, parameters of which should be filled
    # by fleet itself, which is possible since fleet moving to nix repl execution.
    data,
    nixpkgs,
    overlays ? [],
    hosts,
    fleetModules,
    nixosModules ? [],
    extraFleetLib ? {},
  }: let
    hostNames = nixpkgs.lib.attrNames hosts;
    fleetLib =
      (import ./fleetLib.nix {
        inherit nixpkgs hostNames;
      })
      // extraFleetLib;
  in let
    root = nixpkgs.lib.evalModules {
      modules =
        (import ../modules/fleet/_modules.nix)
        ++ [
          data
          ({...}: {
            inherit nixosModules hosts;
            overlays = [(final: prev: (fleetPkgsForPkgs final))] ++ overlays;
          })
        ]
        ++ fleetModules;
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
    }: {
      config = root.config;
    };
    defaultData = withData {
      inherit data;
      root = checkedRoot;
    };
    uncheckedData = withData {inherit data root;};
  in {
    inherit nixpkgs overlays;
    inherit (defaultData) config;
    unchecked = {
      inherit (uncheckedData) config;
    };
  };
}
