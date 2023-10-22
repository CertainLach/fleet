# Tied to build_systems.rs
{config, ...}: {
  # TODO: Make it work with systemd-initrd approach.
  # In this case we can't just switch generation and re-run activation script, since the root filesystem might not be
  # mounted yet. We need to explicitly remove the last generation, and this needs deeper integration with systemd/grub/
  # whatever user uses. boot.json also might help here.

  systemd.services.rollback-watchdog = {
    description = "Rollback watchdog";
    script = ''
      set -eux
      if [ -f /etc/fleet_rollback_marker ]; then
        echo "found the rollback marker, switching to older generation"
        target=$(cat /etc/fleet_rollback_marker)
        echo "rolling back profile"
        nix profile rollback --profile /nix/var/nix/profiles/system --to "$target"
        echo "executing activation script"
        "/nix/var/nix/profiles/system-$target-link/bin/switch-to-configuration" switch || true
        echo "removing rollback marker"
        rm -f /etc/fleet_rollback_marker
      else
        echo "rollback marker was removed, upgrade is succeeded"
      fi
    '';
    path = [
      # Should have nix-command support
      config.nix.package
    ];
    serviceConfig.Type = "exec";
    unitConfig = {
      X-StopOnRemoval = false;
      X-RestartIfChanged = false;
      X-StopIfChanged = false;
    };
  };

  systemd.timers.rollback-watchdog = {
    description = "Timer for rollback watchdog";
    wantedBy = ["timers.target"];
    timerConfig = {
      OnActiveSec = "3min";
      RemainAfterElapse = false;
    };
    unitConfig = {
      ConditionPathExists = "/etc/fleet_rollback_marker";
    };
  };
}
