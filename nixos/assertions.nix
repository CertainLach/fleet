# Similar module exists for fleet, however it also defines assertions and warnings,
# which are already defined for nixos.
{
  lib,
  config,
  ...
}: let
  inherit (lib.options) mkOption;
  inherit (lib.lists) map filter;
  inherit (lib.types) listOf str;
in {
  options = {
    errors = mkOption {
      type = listOf str;
      internal = true;
      description = ''
        Similar to warnings, however build will fail if any error exists.
      '';
    };
  };
  config.errors =
    map (v: v.message)
    (filter (v: !v.assertion) config.assertions);
}
