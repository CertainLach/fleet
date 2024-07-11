{
  lib,
  config,
  pkgs,
  ...
}: let
  inherit (lib.strings) hasPrefix removePrefix;
  inherit (lib) mkOption mkOptionDefault mapAttrs stringAfter;
  inherit (lib.types) submodule str attrsOf nullOr unspecified lazyAttrsOf;
  plaintextPrefix = "<PLAINTEXT>";
  plaintextNewlinePrefix = "<PLAINTEXT-NL>";

  sysConfig = config;
  secretPartType = secretName:
    submodule ({config, ...}: {
      options = {
        raw = mkOption {
          description = "Secret in fleet-specific undocumented format, do not use. Import from fleet.nix";
          internal = true;
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
      config = let
        partName = config._module.args.name;
      in {
        hash = mkOptionDefault (builtins.hashString "sha1" config.raw);
        data = mkOptionDefault (
          if hasPrefix plaintextPrefix config.raw
          then removePrefix plaintextPrefix config.raw
          else if hasPrefix plaintextNewlinePrefix config.raw
          then removePrefix plaintextNewlinePrefix config.raw
          else throw "secret.part.data attribute only works for public plaintext secret parts, got ${config.raw}"
        );
        path = mkOptionDefault "/run/secrets/${secretName}/${config.hash}-${partName}";
        stablePath = mkOptionDefault "/run/secrets/${secretName}/${partName}";
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
      expectedOwners = mkOption {
        type = nullOr unspecified;
        default = null;
        internal = true;
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

      # FIXME: Some of those removed attributes shouldn't be here, but there is some error in passing shared secrets from fleet to nixos.
      "expectedOwners"
    ]));
  secretsFile = pkgs.writeTextFile {
    name = "secrets.json";
    text =
      builtins.toJSON (mapAttrs (_: processSecret)
        config.secrets);
  };
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
    system.activationScripts.decryptSecrets =
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
        ++ (lib.optional (config.system.activationScripts ? "persist-files") "persist-files")
      ) ''
        1>&2 echo "setting up secrets"
        ${pkgs.fleet-install-secrets}/bin/fleet-install-secrets install ${secretsFile}
      '';
  };
}
