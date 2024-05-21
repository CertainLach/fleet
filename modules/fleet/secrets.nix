{
  lib,
  fleetLib,
  config,
  ...
}:
with lib;
with fleetLib; let
  sharedSecret = with types; ({config, ...}: {
    freeformType = types.lazyAttrsOf unspecified;
    options = {
      expectedOwners = mkOption {
        type = nullOr (listOf str);
        description = ''
          List of hosts to encrypt secret for. null if managed by user (= via owners field from fleet.nix)

          Secrets would be decrypted and stored to /run/secrets/$\{name} on owners
        '';
        default = null;
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
        default = [];
      };
    };
  });
  hostSecret = with types; {
    freeformType = types.lazyAttrsOf unspecified;
    options = {
      createdAt = mkOption {
        type = nullOr str;
        default = null;
      };
      expiresAt = mkOption {
        type = nullOr str;
        default = null;
      };
    };
  };
in {
  options = with types; {
    version = mkOption {
      type = str;
      default = "";
      internal = true;
    };
    sharedSecrets = mkOption {
      type = attrsOf (submodule sharedSecret);
      default = {};
      description = "Shared secrets";
    };
    hostSecrets = mkOption {
      type = attrsOf (attrsOf (submodule hostSecret));
      default = {};
      description = "Host secrets. Imported from fleet.nix";
      internal = true;
    };
  };
  config = {
    assertions =
      mapAttrsToList
      (name: secret: {
        assertion = secret.expectedOwners == null || builtins.sort (a: b: a < b) secret.owners == builtins.sort (a: b: a < b) secret.expectedOwners;
        message = "Shared secret ${name} is expected to be encrypted for ${builtins.toJSON secret.expectedOwners}, but it is encrypted for ${builtins.toJSON secret.owners}. Run fleet secrets regenerate to fix";
      })
      config.sharedSecrets;
    hosts = hostsToAttrs (host: {
      nixosModules = let
        # processPart
        processSecret = v:
          (removeAttrs v ["createdAt" "expiresAt" "expectedOwners" "owners" "regenerateOnOwnerAdded" "regenerateOnOwnerRemoved"])
          // {
            shared = true;
          };
      in [
        {
          secrets =
            (
              mapAttrs (_: processSecret)
              (filterAttrs (_: v: builtins.elem host v.owners) config.sharedSecrets)
            )
            // (mapAttrs (_: processSecret) (config.hostSecrets.${host} or {}));
        }
      ];
    });
    # TODO: Should this attribute be moved to `nixpkgs.overlays`?
    overlays = [
      (final: prev: let
        lib = final.lib;
        inherit (lib) strings concatMap;
        inherit (strings) escapeShellArgs;
      in {
        mkEncryptSecret = {
          rage ? prev.rage,
          recipients,
        }:
          prev.writeShellScript "encryptor" ''
            #!/bin/sh
            exec ${rage}/bin/rage ${escapeShellArgs (concatMap (r: ["-r" r]) recipients)} -e "$@"
          '';
        # TODO: Move to fleet
        # TODO: Merge both generators to one with consistent options syntax?
        # Impure generator is built on local machine, then built closure is copied to remote machine,
        # and then it is ran in inpure context, so that this generator may access HSMs and other things.
        mkImpureSecretGenerator = {
          script,
          # If set - script will be run on remote machine, otherwise it will be run with fleet project in CWD
          # (Some secrets-encryption-in-git/managed PKI solution is expected)
          impureOn ? null,
        }:
          (prev.writeShellScript "impureGenerator.sh" ''
            #!/bin/sh
            set -eu

            # TODO: Provide tempdir from outside, to make it securely erasurable as needed?
            tmp=$(mktemp -d)
            cd $tmp
            # cd /var/empty

            created_at=$(date -u +"%Y-%m-%dT%H:%M:%S.%NZ")

            ${script}

            if ! test -d $out; then
              echo "impure generator script did not produce expected \$out output"
              exit 1
            fi

            echo -n $created_at > $out/created_at
            echo -n SUCCESS > $out/marker
          '')
          .overrideAttrs (old: {
            passthru = {
              inherit impureOn;
              generatorKind = "impure";
            };
          });
        # Pure generators are disabled for now
        mkSecretGenerator = {script}: final.mkImpureSecretGenerator {inherit script;};

        # TODO: Implement consistent naming
        # Pure secret generator is supposed to be run entirely by nix, using `__impure` derivation type...
        # But for now, it is ran the same way as `impureSecretGenerator`, but on the local machine.
        # mkSecretGenerator = {script}:
        #   (prev.writeShellScript "generator.sh" ''
        #     #!/bin/sh
        #     set -eu
        #     # TODO: make nix daemon build secret, not just the script.
        #     cd /var/empty
        #
        #     created_at=$(date -u +"%Y-%m-%dT%H:%M:%S.%NZ")
        #
        #     ${script}
        #     if ! test -d $out; then
        #       echo "impure generator script did not produce expected \$out output"
        #       exit 1
        #     fi
        #
        #     echo -n $created_at > $out/created_at
        #     echo -n SUCCESS > $out/marker
        #   '')
        #   .overrideAttrs (old: {
        #     passthru = {
        #       generatorKind = "pure";
        #     };
        #     # TODO: make nix daemon build secret, not just the script.
        #     # __impure = true;
        #   });
      })
    ];
  };
}
