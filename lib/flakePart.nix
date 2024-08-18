{crane}: {
  fleetLib,
  lib,
  config,
  inputs ? {},
  ...
}: let
  inherit (lib.options) mkOption;
  inherit (lib.attrsets) mapAttrs;
  inherit (lib.types) lazyAttrsOf deferredModule unspecified;
  inherit (lib.strings) isPath;
  inherit (fleetLib.options) mkHostsOption;
in {
  options.fleetModules = mkOption {
    type = lazyAttrsOf unspecified;
    default = {};
  };
  options.fleetConfigurations = mkOption {
    type = lazyAttrsOf deferredModule;
    apply = nameToModule:
      mapAttrs (
        name: module: data: let
          # To use user-provided nixpkgs, we first need to extract wanted nixpkgs attribute,
          # to do that, evaluate all the modules with only needed option declared.
          bootstrapEval = lib.evalModules {
            modules = [
              module
              {
                options.nixpkgs.buildUsing = mkOption {
                  description = ''
                    Nixpkgs to use for fleetConfiguration evaluation.
                  '';
                };
                config._module.check = false;
              }
            ];
          };
          bootstrapNixpkgs = bootstrapEval.config.nixpkgs.buildUsing;
          normalEval = bootstrapNixpkgs.lib.evalModules {
            modules =
              (import ../modules/module-list.nix)
              ++ [
                module
                {
                  options.hosts = mkHostsOption {
                    nixos.nixpkgs.overlays = [
                      (final: prev:
                        import ../pkgs {
                          inherit (prev) callPackage;
                          craneLib = crane.mkLib prev;
                        })
                    ];
                  };
                  config = {
                    data =
                      if isPath data
                      then import data
                      else data;
                  };
                }
              ];
            specialArgs = {
              fleetLib = import ../lib {
                inherit (bootstrapNixpkgs) lib;
              };
              inputs = inputs;
            };
          };
        in
          normalEval
      )
      nameToModule;
  };
  config = {
    _module.args.fleetLib = import ../lib {inherit lib;};
    flake.fleetConfigurations = config.fleetConfigurations;
    flake.fleetModules = config.fleetModules;
  };

  _file = ./flakePart.nix;
}
