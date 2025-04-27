# Required for nix copy in build_systems.rs
{
  lib,
  config,
  ...
}:
let
  inherit (lib.modules) mkIf;
  hasPersistentHostname = config.networking.hostName != "";
in
{
  # https://github.com/NixOS/nix/issues/3023
  systemd.services.generate-nix-cache-key = mkIf hasPersistentHostname {
    wantedBy = [ "multi-user.target" ];
    serviceConfig.Type = "oneshot";
    path = [ config.nix.package ];
    script = ''
      [[ -f /etc/nix/private-key ]] && exit
      nix-store --generate-binary-cache-key ${config.networking.hostName}-1 /etc/nix/private-key /etc/nix/public-key
    '';
  };
  nix.settings.secret-key-files = mkIf hasPersistentHostname "/etc/nix/private-key";
}
