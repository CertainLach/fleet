# Shared functions for fleet configuration, available as `fleet` module argument
{
  nixpkgs,
  hostNames,
}: let
  inherit (nixpkgs) lib;
  inherit (lib) listToAttrs remove unique crossLists sort elemAt mkOptionType mkOverride optionalString;
  inherit (lib.types) listOf coercedTo oneOf submodule;
in rec {
  hostsToAttrs = f:
    listToAttrs (
      map (name: {
        inherit name;
        value = f name;
      })
      hostNames
    );
  hostsCartesian = remove null (
    unique (
      crossLists
      (
        a: b:
          if a == b
          then null
          else hostsPair a b
      ) [hostNames hostNames]
    )
  );
  hostsPair = this: other: let
    sorted = sort (a: b: a < b) [this other];
  in {
    a = elemAt sorted 0;
    b = elemAt sorted 1;
  };
  hostPairName = this: other:
    if this < other
    then "${this}-${other}"
    else "${other}-${this}";

  types = rec {
    anyModule = mkOptionType {
      name = "submodule";
      inherit (submodule {}) check;
      merge = lib.options.mergeOneOption;
      description = "Nixos module";
    };
    listOfAnyModuleStrict =
      listOf anyModule;
    listOfAnyModule =
      coercedTo (oneOf [listOfAnyModuleStrict anyModule]) (
        v:
          if builtins.isAttrs v
          then [v]
          else if builtins.isFunction v
          then [v]
          else v
      )
      listOfAnyModuleStrict;
  };

  # mkDefault = mkOverride 1000
  # For places, where fleet knows better than nixpkgs defaults.
  mkFleetDefault = mkOverride 999;
  # Some generators use mkDefault, but optionDefault is set by nixpkgs.
  mkFleetGeneratorDefault = mkOverride 1001;

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
}
