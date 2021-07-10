{ config, lib, fleet, ... }: with lib; with fleet; let
  cfg = config.networking.wireguard;
  genWgKey = { owners }: {
    inherit owners;
    generator = mkSecret (
      { pkgs, encryptCmd }: {
        utils = [ pkgs.wireguard-tools ];
        script = ''
          key=$(wg genkey)
          pub=$(echo $key | wg pubkey)

          mkdir -p $out
          echo $key | ${encryptCmd} >$out/key
          echo $pub >$out/pub_key
        '';
      }
    );
  };
  genWgPsk = { owners }: {
    inherit owners;
    generator = mkSecret (
      { pkgs, encryptCmd }: {
        utils = [ pkgs.wireguard-tools ];
        script = ''
          key=$(wg genpsk)

          mkdir -p $out
          echo $key | ${encryptCmd} >$out/key
        '';
      }
    );
  };

  hostKeys = listToAttrs (
    map
      (
        hostName: {
          name = "wg-key-${hostName}";
          value = genWgKey {
            owners = [ hostName ];
          };
        }
      )
      hostNames
  );
  psks = listToAttrs (
    map
      (
        { a, b }: {
          name = "wg-psk-${a}-${b}";
          value = genWgPsk {
            owners = [ a b ];
          };
        }
      )
      hostsCartesian
  );
in
{
  options.networking.wireguard = with types; {
    enable = mkEnableOption "wireguard";
    interface = mkOption {
      type = str;
      description = "Interface name for wireguard network";
      default = "fleet";
    };
    port = mkOption {
      type = int;
      description = "Port, on which wireguard interface should listen";
      default = 51871;
    };
    allowedIPs = mkOption {
      type = attrsOf (listOf str);
      description = "Per host allowed ips";
    };
  };
  config = mkIf cfg.enable {
    secrets =
      (hostKeys // psks);
    hosts = hostsToAttrs (
      hostName: {
        modules = [
          {
            networking.wireguard.enable = true;
            networking.wireguard.interfaces.fleetwg = {
              privateKeyFile = "/run/secrets/wg-key-${hostName}";
              peers = map
                (
                  peer:
                  let
                    pair = hostsPair hostName peer;
                  in
                  {
                    publicKey = config.secrets."wg-key-${peer}".data.key;
                    presharedKey = "/run/secrets/wg-psk-${pair.a}-${pair.b}";
                    allowedIPs = cfg.allowedIPs.${peer};
                  }
                )
                hostNames;
            };
          }
        ];
      }
    );
  };
}
