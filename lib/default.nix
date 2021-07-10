{
  fleetConfiguration = { nixpkgs, hosts, ... }@allConfig:
    let
      config = builtins.removeAttrs allConfig [ "nixpkgs" ];
    in
    rec {
      root = nixpkgs.lib.evalModules {
        modules =
          (import ../modules/modules.nix) ++ [
            config
            (
              { ... }: {
                options = { };
                config = {
                  # Secret data is available only via fleet build-systems
                  secrets =
                    if builtins?getEnv then
                      let
                        stringData = builtins.getEnv "SECRET_DATA";
                      in
                      if stringData != "" then (builtins.fromJSON stringData) else { }
                    else { };
                };
              }
            )
          ];
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
              inherit name; value = nixpkgs.lib.nixosSystem {
              system = configuredHosts.${name}.system;
              modules = configuredHosts.${name}.modules;
            };
            }
          )
          (builtins.attrNames root.config.hosts)
      ); #nixpkgs.lib.nixosSystem {}
    };
}
