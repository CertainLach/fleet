{
  config,
  lib,
  inputs,
  ...
}: let
  inherit (lib) mkOption;
  inherit (lib.types) deferredModule;
in {
  options.tf = mkOption {
    type = deferredModule;
    apply = module: system:
      inputs.terranix.lib.terranixConfigurationAst {
        inherit system;
        pkgs = config.nixpkgs.buildUsing.legacyPackages.${system};
        modules = [module];
      };
  };
  config.tf.output.fleet = {
    value = {
      managed = true;
    };
    # Just to avoid printing this attribute on every apply.
    sensitive = true;
  };
}
