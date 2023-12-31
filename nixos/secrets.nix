{ lib, config, pkgs, ... }:

with lib;

let
  sysConfig = config;
  secretType = types.submodule ({ config, ... }: {
    config = let secretName = config._module.args.name; in {
      stableSecretPath = mkOptionDefault "/run/secrets/secret-stable-${secretName}";
      secretPath = mkOptionDefault "/run/secrets/secret-${config.secretHash}-${secretName}";
      secretHash = mkOptionDefault (if config.secret != null then (builtins.hashString "sha1" config.secret) else throw "secret is not defined for secret ${secretName}");

      stablePublicPath = mkOptionDefault "/run/secrets/public-stable-${secretName}";
      publicPath = mkOptionDefault "/run/secrets/public-${config.publicHash}-${secretName}";
      publicHash = mkOptionDefault (if config.public != null then (builtins.hashString "sha1" config.public) else throw "public is not defined for secret ${secretName}");
    };
    options = with types; {
      shared = mkOption {
        description = "Is this secret owned by this machine, or propagated from shared secrets";
        default = false;
      };

      generator = mkOption {
        type = nullOr unspecified;
        description = "Derivation to evaluate for secret generation";
        default = null;
      };

      public = mkOption {
        type = nullOr str;
        description = "Secret public data";
        default = null;
      };
      secret = mkOption {
        type = nullOr str;
        description = "Encrypted secret data";
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

      secretHash = mkOption {
        type = str;
        description = "Hash of .secret field";
      };
      publicHash = mkOption {
        type = str;
        description = "Hash of .public field";
      };

      stableSecretPath = mkOption {
        type = str;
        description = ''
          Use this, if target process supports re-reading of secret from disk,
          and doesn't needs to be restarted when secret is updated in file
        '';
      };
      secretPath = mkOption {
        type = str;
        description = "Path to decrypted secret, suffixed with contents hash";
      };

      stablePublicPath = mkOption {
        type = str;
        description = ''
          Use this, if target process supports re-reading of secret from disk,
          and doesn't needs to be restarted when secret is updated in file
        '';
      };
      publicPath = mkOption {
        type = str;
        description = "Path to the public part of secret";
      };
    };
  });
  secretsFile = pkgs.writeTextFile {
    name = "secrets.json";
    text = builtins.toJSON (mapAttrs (_: value: rec {
      inherit (value) group mode owner secret public;
      publicPath = if public != null then value.publicPath else "/missingno";
      stablePublicPath = if public != null then value.stablePublicPath else "/missingno";
      secretPath = if secret != null then value.secretPath else "/missingno";
      stableSecretPath = if secret != null then value.stableSecretPath else "/missingno";
    }) config.secrets);
  };
in
{
  options = {
    secrets = mkOption {
      type = types.attrsOf secretType;
      default = { };
      description = "Host-local secrets";
    };
  };
  config = {
    environment.systemPackages = with pkgs; [pkgs.fleet-install-secrets];
    system.activationScripts.decryptSecrets = stringAfter [ "users" "groups" "specialfs" ] ''
      1>&2 echo "setting up secrets"
      ${pkgs.fleet-install-secrets}/bin/fleet-install-secrets install ${secretsFile}
    '';
  };
}
