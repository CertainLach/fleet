# Shared functions for fleet configuration, available as `fleet` module argument
{lib}: let
  inherit (lib.trivial) isFunction;
  inherit (lib.options) mkOption mergeOneOption;
  inherit (lib.modules) mkOverride;
  inherit (lib.types) listOf submodule attrsOf mkOptionType;
  inherit (lib.strings) optionalString;
in rec {
  types = {
    overlay = mkOptionType {
      name = "nixpkgs-overlay";
      description = "nixpkgs overlay";
      check = isFunction;
      merge = mergeOneOption;
    };
    listOfOverlay = listOf types.overlay;

    mkHostsType = module: attrsOf (submodule module);
  };

  options = {
    mkHostsOption = module:
      mkOption {
        type = types.mkHostsType module;
      };
  };

  inherit (options) mkHostsOption;

  modules = {
    # mkDefault = mkOverride 1000
    # For places, where fleet knows better than nixpkgs defaults.
    mkFleetDefault = mkOverride 999;
    # Some generators use mkDefault, but optionDefault is set by nixpkgs.
    mkFleetGeneratorDefault = mkOverride 1001;
  };

  inherit (modules) mkFleetDefault mkFleetGeneratorDefault;

  secrets = {
    mkPassword = {size ? 32}: {
      coreutils,
      mkSecretGenerator,
      ...
    }:
      mkSecretGenerator {
        script = ''
          mkdir $out
          gh generate password -o $out/secret --size ${toString size}
        '';
      };

    mkEd25519 = {
      noEmbedPublic ? false,
      encoding ? null,
    }: {mkSecretGenerator, ...}:
      mkSecretGenerator {
        script = ''
          mkdir $out
          gh generate ed25519 -p $out/public -s $out/secret \
            ${optionalString noEmbedPublic "--no-embed-public"} \
            ${optionalString (encoding != null) "--encoding=${encoding}"}
        '';
      };

    mkX25519 = {encoding ? null}: {mkSecretGenerator, ...}:
      mkSecretGenerator {
        script = ''
          mkdir $out
          gh generate x25519 -p $out/public -s $out/secret \
            ${optionalString (encoding != null) "--encoding=${encoding}"}
        '';
      };

    mkRsa = {size ? 4096}: {
      openssl,
      mkSecretGenerator,
      ...
    }:
      mkSecretGenerator {
        script = ''
          mkdir $out

          ${openssl}/bin/openssl genrsa -out rsa_private.key ${toString size}
          ${openssl}/bin/openssl rsa -in rsa_private.key -pubout -out rsa_public.key

          cat rsa_private.key | gh private -o $out/secret
          cat rsa_public.key | gh public -o $out/public
        '';
      };

    mkBytes = {
      count ? 32,
      encoding,
      noNuls ? false,
    }: {mkSecretGenerator, ...}:
      mkSecretGenerator {
        script = ''
          mkdir $out
          gh generate bytes --count=${toString count} --encoding=${encoding} -o $out/secret \
            ${optionalString noNuls "--no-nuls"}
        '';
      };
    mkHexBytes = {count ? 32}:
      mkBytes {
        inherit count;
        encoding = "hex";
      };
    mkBase64Bytes = {count ? 32}:
      mkBytes {
        inherit count;
        encoding = "base64";
      };

    # Wireguard
    # mkWireguard = {}: mkX25519 {encoding = "base64";};
    # mkWireguardPsk = {}: mkBase64Bytes {count = 32;};
  };

  inherit (secrets) mkPassword mkEd25519 mkX25519 mkRsa mkBytes mkHexBytes mkBase64Bytes;
}
