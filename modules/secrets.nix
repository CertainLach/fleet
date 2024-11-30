{
  lib,
  config,
  ...
}: let
  inherit (lib.options) mkOption literalExpression;
  inherit (lib.types) unspecified nullOr listOf str bool attrsOf submodule;
  inherit (lib.strings) concatStringsSep;
  inherit (lib.attrsets) mapAttrs;

  sharedSecret = {config, ...}: {
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
        defaultText = literalExpression "regenerateOnOwnerAdded";
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
      expectedGenerationData = mkOption {
        type = unspecified;
        description = "Data that gets embedded into secret part";
        default = null;
      };
    };
  };
in {
  options = {
    sharedSecrets = mkOption {
      type = attrsOf (submodule sharedSecret);
      default = {};
      description = "Shared secrets";
    };
  };
  config = {
    hosts =
      mapAttrs (_: secretMap: {
        nixos.secrets = mapAttrs (_: s: removeAttrs s ["createdAt" "expiresAt"]) secretMap;
      })
      config.data.hostSecrets;
    nixpkgs.overlays = [
      (final: prev: {
        mkSecretGenerators = {recipients}: rec {
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

              export GENERATOR_HELPER_IDENTITIES="${concatStringsSep "\n" recipients}";
              export PATH=${final.fleet-generator-helper}/bin:$PATH

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
          mkSecretGenerator = {script}: mkImpureSecretGenerator {inherit script;};

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
        };
      })
    ];
  };
}
