{
  lib,
  config,
  ...
}: let
  inherit (lib.options) mkOption;
  inherit (lib.types) listOf unspecified str;
  inherit (lib.lists) map filter;

  errors = mkOption {
    type = listOf str;
    internal = true;
    description = ''
      Similar to warnings, however build will fail if any error exists.
    '';
  };
in {
  options = {
    assertions = mkOption {
      type = listOf unspecified;
      internal = true;
      default = [];
      example = [
        {
          assertion = false;
          message = "you can't enable this for that reason";
        }
      ];
      description = ''
        This option allows modules to express conditions that must
        hold for the evaluation of the system configuration to
        succeed, along with associated error messages for the user.
      '';
    };

    warnings = mkOption {
      internal = true;
      default = [];
      type = listOf str;
      example = ["The `foo' service is deprecated and will go away soon!"];
      description = ''
        This option allows modules to show warnings to users during
        the evaluation of the system configuration.
      '';
    };

    inherit errors;
  };
  config = {
    errors =
      map (v: v.message)
      (filter (v: !v.assertion) config.assertions);

    nixos = {config, ...}: {
      _file = ./assertions.nix;
      options = {
        inherit errors;
      };
      config.errors =
        map (v: v.message)
        (filter (v: !v.assertion) config.assertions);
    };
  };
}
