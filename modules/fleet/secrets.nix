{ lib, fleetLib, config, ... }: with lib; with fleetLib;
let
  sharedSecret = with types; {
    options = {
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
        type = nullOr (submodule {
          packages = mkOption {
            type = attrsOf package;
            description = ''
              Derivation to execute for shared secret generation (key = system).
              This derivation should produce directory, with exactly two files:
                - publicData
                - encryptedSecretData

              If null - secret value may only be created manually.
            '';
          };
          expectedData = mkOption {
            type = types.unspecified;
            description = "Data expected to be used for secret generation, if doesn't match specified - secret should be regenerated";
          };
          dependencies = mkOption {
            type = listOf str;
            description = ''
              List of secrets, on which this secret depends.

              During generation, generator command will be ran on host, which already has specified secrets generated.
            '';
            default = [];
          };
          data = mkOption {
            type = types.unspecified;
            description = "Data used for secret generation. Imported from fleet.nix";
            default = null;
            internal = true;
          };
        });
        default = null;
      };
      expireIn = mkOption {
        type = nullOr int;
        description = "Time in hours, in which this secret should be regenerated";
        default = null;
      };

      owners = mkOption {
        type = listOf str;
        description = ''
          For which owners this secret is currently encrypted,
          if not matches expectedOwners - then this secret is considered outdated, and
          should be regenerated/reencrypted.

          Imported from fleet.nix
        '';
        default = [ ];
      };
      public = mkOption {
        type = nullOr str;
        description = "Secret public data. Imported from fleet.nix";
        default = null;
      };
      secret = mkOption {
        type = nullOr str;
        description = "Encrypted secret data. Imported from fleet.nix";
        default = null;
        internal = true;
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
