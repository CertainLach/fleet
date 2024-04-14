# Shared functions for fleet configuration, available as `fleet` module argument
{
  nixpkgs,
  hostNames,
}:
with nixpkgs.lib; rec {
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

  # mkDefault = mkOverride 1000
  # For places, where fleet knows better than nixpkgs defaults.
  mkFleetDefault = mkOverride 999;
  # Some generators use mkDefault, but optionDefault is set by nixpkgs.
  mkFleetGeneratorDefault = mkOverride 1001;

  mkPassword = {size ? 32}: {
    coreutils,
    encrypt,
    mkSecretGenerator,
  }:
    mkSecretGenerator {
      script = ''
        ${coreutils}/bin/tr -dc 'A-Za-z0-9!?%=' < /dev/random \
          | ${coreutils}/bin/head -c ${toString size} \
          | ${encrypt} > $out/secret
      '';
    };

  mkRsa = {size ? 4096}: {
    openssl,
    encrypt,
    mkSecretGenerator,
  }:
    mkSecretGenerator {
      script = ''
        ${openssl}/bin/openssl genrsa -out rsa_private.key ${toString size}
        ${openssl}/bin/openssl rsa -in rsa_private.key -pubout -out rsa_public.key

        sudo cat rsa_private.key | ${encrypt} > $out/secret
        sudo cat rsa_public.key > $out/public
      '';
    };
}
