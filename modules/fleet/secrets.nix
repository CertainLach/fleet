{ lib, fleetLib, config, ... }: with lib; with fleetLib;
let
  sharedSecret = with types; ({config, ...}: {
    options = {
      expectedOwners = mkOption {
        type = nullOr (listOf str);
        description = ''
          List of hosts to encrypt secret for. null if managed by user (= via owners field from fleet.nix)

          Secrets would be decrypted and stored to /run/secrets/$\{name} on owners
        '';
      };
      # TODO: Aren't those options may be just desugared to data/expectedData?
      regenerateOnOwnerAdded = mkOption {
        type = bool;
        description = ''
          Is this secret owner-dependent, and needs to be regenerated on ownership set change, or it may be just reencrypted.
          
          You want to have this option set to true, when this secret contains some reference to its owners, i.e x509 SANs.
        '';
      };
      regenerateOnOwnerRemoved = mkOption {
        default = config.regenerateOnOwnerAdded;
        type = bool;
        description = ''
          Should this secret be removed on owner removal, or it may be just reencrypted
          
          Most probably its value should be equal to regenerateOnOwnerAdded, override only if you know what are you doing.
          Contrary to regenerateOnOwnerAdded, you may want to set this option to false, when host permissions are revoked
          in some other way than by this secret ownership, I.e by firewall/etc.
        '';
      };
      generator = mkOption {
        type = nullOr unspecified;
        description = "Derivation to evaluate for secret generation";
        default = null;
      };
      createdAt = mkOption {
        type = nullOr str;
        description = "When this secret was (re)generated";
        default = null;
      };
      expiresAt = mkOption {
        type = nullOr str;
        description = "On which date this secret will expire, someone should regenerate this secret before it expires.";
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
      # TODO: Make secret generator generate arbitrary number of secret/public parts?
      # Make it generate a folder, where all files except suffixed by .enc are public, and the rest are secret?
      # How should modules refer to those files then?
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
  });
  hostSecret = with types; {
    options = {
      createdAt = mkOption {
        type = nullOr str;
        default = null;
      };
      expiresAt = mkOption {
        type = nullOr str;
        default = null;
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
      description = "Host secrets. Imported from fleet.nix";
      internal = true;
    };
  };
  config = {
    assertions = mapAttrsToList
      (name: secret: {
        assertion = secret.expectedOwners == null || builtins.sort (a: b: a < b) secret.owners == builtins.sort (a: b: a < b) secret.expectedOwners;
        message = "Shared secret ${name} is expected to be encrypted for ${builtins.toJSON secret.expectedOwners}, but it is encrypted for ${builtins.toJSON secret.owners}. Run fleet secrets regenerate to fix";
      })
      config.sharedSecrets;
    hosts = hostsToAttrs (host: {
      modules =
        let
          cleanupSecret = (secretName: v: {
            inherit (v) public secret;
            shared = true;
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
