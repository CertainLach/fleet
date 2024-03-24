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
}
