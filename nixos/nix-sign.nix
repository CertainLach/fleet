# Required for nix copy in build_systems.rs
{config, ...}: {
  # https://github.com/NixOS/nix/issues/3023
  systemd.services.generate-nix-cache-key = {
    wantedBy = ["multi-user.target"];
    serviceConfig.Type = "oneshot";
    path = [config.nix.package];
    script = ''
      [[ -f /etc/nix/private-key ]] && exit
      nix-store --generate-binary-cache-key ${config.networking.hostName}-1 /etc/nix/private-key /etc/nix/public-key
    '';
  };
  nix.settings.secret-key-files = "/etc/nix/private-key";
}
