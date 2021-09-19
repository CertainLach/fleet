{ lib, fleet, config, ... }: with lib;
let
  secret = with types; {
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
in
{
  options = with types; {
    secrets = mkOption {
      type = attrsOf (submodule secret);
      default = { };
      description = "Secrets";
    };
  };
  config = with fleet; {
    hosts = hostsToAttrs (host: {
      modules = [
        ./nixosModule.nix
        {
          secrets = mapAttrs
            (secretName: v: {
              inherit (v) public secret;
            })
            (filterAttrs (_: v: builtins.elem host v.owners) config.secrets);
        }
      ];
    });
  };
}
