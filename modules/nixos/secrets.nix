{
  lib,
  fleetLib,
  config,
  pkgs,
  ...
}: let
  inherit (builtins) hashString;
  inherit (lib.stringsWithDeps) stringAfter;
  inherit (lib.options) mkOption literalExpression;
  inherit (lib.lists) optional;
  inherit (lib.attrsets) mapAttrs;
  inherit (lib.modules) mkIf;
  inherit (lib.types) submodule str attrsOf nullOr unspecified lazyAttrsOf;
  inherit (fleetLib.strings) decodeRawSecret;

  sysConfig = config;
  secretPartType = secretName:
    submodule ({config, ...}: let
      partName = config._module.args.name;
    in {
      options = {
        raw = mkOption {
          type = str;
          internal = true;
          description = "Encoded & Encrypted secret part data, passed from fleet.nix";
        };
        hash = mkOption {
          type = str;
          description = "Hash of secret in encoded format";
        };
        path = mkOption {
          type = str;
          description = "Path to secret part, incorporating data hash (thus it will be updated on secret change)";
        };
        stablePath = mkOption {
          type = str;
          description = "Path to secret part, incorporating data hash (thus it will be updated on secret change)";
        };
        data = mkOption {
          type = str;
          description = "Secret public data (only available for plaintext)";
        };
      };
      config = {
        hash = hashString "sha1" config.raw;
        data = decodeRawSecret config.raw;
        path = "/run/secrets/${secretName}/${config.hash}-${partName}";
        stablePath = "/run/secrets/${secretName}/${partName}";
      };
    });
  secretType = submodule ({config, ...}: let
    secretName = config._module.args.name;
  in {
    freeformType = lazyAttrsOf (secretPartType secretName);
    options = {
      shared = mkOption {
        description = "Is this secret owned by this machine, or propagated from shared secrets";
        default = false;
      };

      generator = mkOption {
        type = nullOr unspecified;
        description = "Derivation to evaluate for secret generation";
        default = null;
      };
      mode = mkOption {
        type = str;
        description = "Secret mode";
        default = "0440";
      };
      owner = mkOption {
        type = str;
        description = "Owner of the secret";
        default = "root";
      };
      group = mkOption {
        type = str;
        description = "Group of the secret";
        default = sysConfig.users.users.${config.owner}.group;
        defaultText = literalExpression "config.users.users.$${owner}.group";
      };
      expectedGenerationData = mkOption {
        type = unspecified;
        description = "Data that gets embedded into secret part";
        default = null;
      };
    };
  });
  processPart = part: {
    inherit (part) raw path stablePath;
  };
  processSecret = secret:
    {
      inherit (secret) group mode owner;
    }
    // (mapAttrs (_: processPart) (removeAttrs secret [
      "shared"
      "generator"
      "mode"
      "group"
      "owner"
    ]));
  secretsFile = pkgs.writeTextFile {
    name = "secrets.json";
    text =
      builtins.toJSON (mapAttrs (_: processSecret)
        config.secrets);
  };
  useSysusers = (config.systemd ? sysusers && config.systemd.sysusers.enable) || (config ? userborn && config.userborn.enable);
in {
  options = {
    secrets = mkOption {
      type = attrsOf secretType;
      default = {};
      description = "Host-local secrets";
    };
  };
  config = {
    environment.systemPackages = [pkgs.fleet-install-secrets];

    systemd.services.fleet-install-secrets = mkIf useSysusers {
      wantedBy = ["sysinit.target"];
      after = ["systemd-sysusers.service"];
      restartTriggers = [
        secretsFile
      ];
      aliases = [
        "sops-install-secrets"
        "agenix-install-secrets"
      ];

      unitConfig.DefaultDependencies = false;

      serviceConfig = {
        Type = "oneshot";
        RemainAfterExit = true;
        ExecStart = "${pkgs.fleet-install-secrets}/bin/fleet-install-secrets install ${secretsFile}";
      };
    };
    system.activationScripts.decryptSecrets =
      mkIf (!useSysusers)
      (
        stringAfter (
          [
            # secrets are owned by user/group, thus we need to refer to those
            "users"
            "groups"
            "specialfs"
          ]
          # nixos-impermanence compatibility: secrets are encrypted by host-key,
          # but with impermanence we expect that the host-key is installed by
          # persist-file activation script.
          ++ (optional (config.system.activationScripts ? "persist-files") "persist-files")
        ) ''
          1>&2 echo "setting up secrets"
          ${pkgs.fleet-install-secrets}/bin/fleet-install-secrets install ${secretsFile}
        ''
      );
  };
}
