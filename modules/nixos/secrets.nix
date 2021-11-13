{ lib, config, pkgs, ... }: with lib;
let
  sysConfig = config;
  secretType = types.submodule ({ config, ... }: {
    config = {
      path = mkOptionDefault (if config.secret == null then (error "secret is not set") else "/run/secrets/${config._module.args.name}");
      publicPath = mkOptionDefault (pkgs.writeText "pub-${config._module.args.name}" config.public);
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

      path = mkOption {
        type = types.str;
        readOnly = true;
        description = "Path to the decrypted secret";
      };
      publicPath = mkOption {
        type = types.package;
        readOnly = true;
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
    system.activationScripts.decryptSecrets = ''
      1>&2 echo "setting up secrets"
      ${pkgs.fleet-install-secrets}/bin/fleet-install-secrets ${secretsFile}
    '';
  };
}
