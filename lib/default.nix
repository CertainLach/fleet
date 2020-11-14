{
  fleetConfiguration = { common ? { modules = []; }, hosts, nixpkgs }@args:
    rec {
      root = nixpkgs.lib.evalModules {
        modules = [
          (
            { ... }: {
              config = {
                inherit hosts;
                # Secret data is available only via fleet build-systems
                secrets = if builtins?getEnv then
                  let
                    stringData = builtins.getEnv "SECRET_DATA";
                  in
                    if stringData != "" then (builtins.fromJSON stringData) else {}
                else {};
              };

            }
          )
        ] ++ common.modules ++ import ./modules/modules.nix {
          pkgs = nixpkgs;
          lib = nixpkgs.lib;
        };

        specialArgs = {
          fleet = import ./lib/fleetLib.nix {
            inherit nixpkgs hosts;
          };
        };
      };
      configuredHosts = root.config.hosts;
      configuredSecrets = root.config.secrets;
      configuredSystems = listToAttrs (
        map (
          name: {
            inherit name; value = nixpkgs.lib.nixosSystem {
            system = configuredHosts.${name}.system;
            modules = configuredHosts.${name}.modules;
          };
          }
        ) (builtins.attrNames hosts)
      ); #nixpkgs.lib.nixosSystem {}
    };
}
