{ lib, fleet, config, ... }: with lib;
let
  sharedSecret = with types; {
    options = {
      owners = mkOption {
        type = listOf str;
        description = ''
          List of hosts to encrypt secret for

          Secrets would be decrypted and stored to /run/secrets/$\{name} on owners
        '';
      };
      generator = mkOption {
        type = package;
        description = "Derivation to execute for secret generation";
      };
      expireIn = mkOption {
        type = nullOr int;
        description = "Time in hours, in which this secret should be regenerated";
        default = null;
      };
      public = mkOption {
        type = nullOr str;
        description = "Secret public data";
        default = null;
      };
      secret = mkOption {
        type = str;
        description = "Encrypted secret data";
      };
    };
  };
  hostSecret = with types; {
    options = {
      generator = mkOption {
        type = package;
        description = "Derivation to execute for secret generation";
      };
      expireIn = mkOption {
        type = nullOr int;
        description = "Time in hours, in which this secret should be regenerated";
        default = null;
      };
      public = mkOption {
        type = nullOr str;
        description = "Secret public data";
        default = null;
      };
      secret = mkOption {
        type = str;
        description = "Encrypted secret data";
      };
    };
  };
in
{
  options = with types; {
    sharedSecrets = mkOption {
      type = attrsOf (submodule sharedSecret);
      default = { };
      description = "Shared secrets";
    };
    hostSecrets = mkOption {
      type = attrsOf (attrsOf (submodule hostSecret));
      default = { };
      description = "Host secrets";
    };
  };
  config = with fleet; {
    hosts = hostsToAttrs (host: {
      modules =
        let
          cleanupSecret = (secretName: v: {
            inherit (v) public secret;
          });
        in
        [
          {
            secrets = (mapAttrs cleanupSecret
              (filterAttrs (_: v: builtins.elem host v.owners) config.sharedSecrets)
            ) // (mapAttrs cleanupSecret (config.hostSecrets.${host} or {}));
          }
        ];
    });
  };
}
