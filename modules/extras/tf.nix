{
  config,
  lib,
  fleetLib,
  inputs,
  ...
}:
let
  inherit (lib.options) mkOption;
  inherit (lib.types) deferredModule attrsOf unspecified;
  inherit (fleetLib.options) mkDataOption;
in
{
  options = {
    tf = mkOption {
      type = deferredModule;
      apply =
        module: system:
        inputs.terranix.lib.terranixConfiguration {
          inherit system;
          pkgs = config.nixpkgs.buildUsing.legacyPackages.${system};
          modules = [
            module
          ];
        };
    };
    data = mkDataOption {
      # host => hostData
      options.extra.terraformHosts = mkOption {
        default = { };
        type = attrsOf (attrsOf unspecified);
        description = "Hosts data provided by fleet tf";
      };
    };
  };

  config = {
    tf.output.fleet = {
      value = {
        managed = true;
      };
      # Just to avoid printing this attribute on every apply, the whole fleet attribute
      # will be somehow processed by fleet tf.
      sensitive = true;
    };
    hosts = config.data.extra.terraformHosts;
  };
}
