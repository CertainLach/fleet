{ lib, ... }: with lib;
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
        type = types.package;
        description = "Derivation to execute for secret generation";
      };
      expireIn = mkOption {
        type = nullOr int;
        description = "Time in hours, in which this secret should be regenerated";
        default = null;
      };
      data = mkOption {
        type = attrsOf anything;
        description = "Generated secret data, do not set it yourself";
        default = { };
      };
    };
  };
  host = with types; {
    options = {
      modules = mkOption {
        type = listOf anything;
        description = "List of nixos modules";
        default = [ ];
      };
      network = mkOption {
        type = submodule {
          options = {
            fleetIp = {
              type = str;
              description = "Ip which is available to all hosts in fleet";
            };
          };
        };
        description = "Network definition of host";
      };
      system = mkOption {
        type = str;
        description = "Type of system";
      };
    };
  };
in
{
  options = with types; {
    hosts = mkOption {
      type = attrsOf (submodule host);
      default = { };
      description = "Configurations of individual hosts";
    };
    secrets = mkOption {
      type = attrsOf (submodule secret);
      default = { };
      description = "Secrets";
    };
  };
  config = {
    secrets =
      if builtins?getEnv then
        let
          stringData = builtins.getEnv "SECRET_DATA";
        in
        if stringData != "" then (builtins.fromJSON stringData) else { }
      else { };
  };
}
