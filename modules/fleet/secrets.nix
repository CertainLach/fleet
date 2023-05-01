{ lib, fleetLib, config, ... }: with lib; with fleetLib;
let
  sharedSecret = with types; {
    options = {
      owners = mkOption {
        type = listOf str;
        description = ''
          For which owners this secret is currently encrypted,
          if not matches expectedOwners - then this secret is considered outdated, and
          should be regenerated/reencrypted
        '';
        default = [ ];
      };
      expectedOwners = mkOption {
        type = listOf str;
        description = ''
          List of hosts to encrypt secret for

          Secrets would be decrypted and stored to /run/secrets/$\{name} on owners
        '';
        default = [ ];
      };
      ownerDependent = mkOption {
        type = bool;
        description = "Is this secret owner-dependent, and needs to be regenerated on ownership set change, or it may be just reencrypted";
      };
      generator = mkOption {
        type = nullOr package;
        description = ''
          Derivation to execute for secret generation

          If null - may only be created manually
        '';
        default = null;
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
        type = nullOr str;
        description = "Encrypted secret data";
        default = null;
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
  config = {
    assertions = mapAttrsToList
      (name: secret: {
        assertion = builtins.sort (a: b: a < b) secret.owners == builtins.sort (a: b: a < b) secret.expectedOwners;
        message = "Shared secret ${name} is expected to be encrypted for ${builtins.toJSON secret.expectedOwners}, but it is encrypted for ${builtins.toJSON secret.owners}. Run fleet secrets regenerate to fix";
      })
      config.sharedSecrets;
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
            ) // (mapAttrs cleanupSecret (config.hostSecrets.${host} or { }));
          }
        ];
    });
  };
}
