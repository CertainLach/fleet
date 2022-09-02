{ lib, config, pkgs, ... }:

with lib;

let
  sysConfig = config;
  secretType = types.submodule ({ config, ... }: {
    config = rec {
      stableSecretPath = mkOptionDefault "/run/secrets/secret-stable-${config._module.args.name}";
      secretPath = mkOptionDefault "/run/secrets/secret-${config.secretHash}-${config._module.args.name}";
      secretHash = mkOptionDefault (if config.secret != null then (builtins.hashString "sha1" config.secret) else "<missingno>");

      stablePublicPath = mkOptionDefault "/run/secrets/public-stable-${config._module.args.name}";
      publicPath = mkOptionDefault "/run/secrets/public-${config.publicHash}-${config._module.args.name}";
      publicHash = mkOptionDefault (if config.public != null then (builtins.hashString "sha1" config.public) else "<missingno>");
    };
    options = {
      public = mkOption {
        type = types.nullOr types.str;
        description = "Secret public data";
        default = null;
      };
      secret = mkOption {
        type = types.nullOr types.str;
        description = "Encrypted secret data";
        default = null;
      };
      mode = mkOption {
        type = types.str;
        description = "Secret mode";
        default = "0440";
      };
      owner = mkOption {
        type = types.str;
        description = "Owner of the secret";
        default = "root";
      };
      group = mkOption {
        type = types.str;
        description = "Group of the secret";
        default = sysConfig.users.users.${config.owner}.group;
      };

      secretHash = mkOption {
        type = types.str;
        description = "Hash of .secret field";
      };
      publicHash = mkOption {
        type = types.str;
        description = "Hash of .public field";
      };

      stableSecretPath = mkOption {
        type = types.str;
        description = ''
          Use this, if target process supports re-reading of secret from disk,
          and doesn't needs to be restarted when secret is updated in file
        '';
      };
      secretPath = mkOption {
        type = types.str;
        description = "Path to decrypted secret, suffixed with contents hash";
      };

      stablePublicPath = mkOption {
        type = types.str;
        description = ''
          Use this, if target process supports re-reading of secret from disk,
          and doesn't needs to be restarted when secret is updated in file
        '';
      };
      publicPath = mkOption {
        type = types.str;
        description = "Path to the public part of secret";
      };
    };
  });
  secretsFile = pkgs.writeTextFile {
    name = "secrets.json";
    text = builtins.toJSON config.secrets;
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
    system.activationScripts.decryptSecrets = stringAfter [ "users" "groups" "specialfs" ] ''
      1>&2 echo "setting up secrets"
      ${pkgs.fleet-install-secrets}/bin/fleet-install-secrets ${secretsFile}
    '';
  };
}
