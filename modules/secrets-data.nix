{
  lib,
  fleetLib,
  config,
  ...
}: let
  inherit (fleetLib.options) mkDataOption;
  inherit (lib.options) mkOption;
  inherit (lib.types) lazyAttrsOf nullOr listOf str attrsOf submodule bool;
  inherit (lib.attrsets) mapAttrsToList mapAttrs catAttrs filterAttrs genAttrs;
  inherit (lib.lists) sort unique concatLists;
  inherit (lib.strings) toJSON;

  secretDataValue = {
    options = {
      raw = mkOption {
        type = nullOr str;
        description = "Encrypted + encoded secret data";
        default = null;
      };
    };
  };

  sharedSecretData = {
    freeformType = attrsOf (submodule secretDataValue);
    options = {
      createdAt = mkOption {
        type = str;
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
        default = [];
      };
    };
  };

  hostSecretData = {
    freeformType = attrsOf (submodule secretDataValue);
    options = {
      createdAt = mkOption {
        type = str;
        description = "When this secret was (re)generated";
        default = null;
      };
      expiresAt = mkOption {
        type = nullOr str;
        description = "On which date this secret will expire, someone should regenerate this secret before it expires.";
        default = null;
      };
      shared = mkOption {
        type = bool;
        description = "On which date this secret will expire, someone should regenerate this secret before it expires.";
        default = false;
      };
    };
  };
in {
  options.data = mkDataOption ({config, ...}: {
    options = {
      sharedSecrets = mkOption {
        type = attrsOf (submodule sharedSecretData);
        default = {};
        description = "Stored shared secret data.";
      };
      hostSecrets = mkOption {
        type = attrsOf (attrsOf (submodule hostSecretData));
        default = {};
        description = "Host secrets.";
        internal = true;
      };
    };
    config.hostSecrets = let
      hostsWithSharedSecrets = unique (concatLists (mapAttrsToList (_: s: s.owners) config.sharedSecrets));
      secretsHavingHost = host: filterAttrs (_: secret: lib.elem host secret.owners) config.sharedSecrets;
      toHostSecret = _: secret: (removeAttrs secret ["owners"]) // {shared = true;};
    in
      genAttrs hostsWithSharedSecrets (host: mapAttrs toHostSecret (secretsHavingHost host));
  });
  config = {
    assertions =
      mapAttrsToList
      (name: secret: {
        assertion = secret.expectedOwners == null || sort (a: b: a < b) config.data.sharedSecrets.${name}.owners == sort (a: b: a < b) secret.expectedOwners;
        message = "Shared secret ${name} is expected to be encrypted for ${toJSON secret.expectedOwners}, but it is encrypted for ${toJSON config.data.sharedSecrets.${name}.owners}. Run fleet secrets regenerate to fix";
      })
      config.sharedSecrets;
    sharedSecrets =
      mapAttrs (_: _: {}) config.data.sharedSecrets;
  };
}
