# Shared functions for fleet configuration, available as `fleet` module argument
{ nixpkgs, hosts }: with nixpkgs.lib; rec {
  # Modules can't register hosts because of infinite recursion
  hostNames = attrNames hosts;
  hostsToAttrs = f: listToAttrs (
    map (name: { inherit name; value = f name; }) hostNames
  );
  hostsCartesian = remove null (
    unique (
      crossLists
        (
          a: b:
            if a == b then
              null
            else
              hostsPair a b
        ) [ hostNames hostNames ]
    )
  );
  hostsPair = this: other:
    let
      sorted = sort (a: b: a < b) [ this other ];
    in
    {
      a = elemAt sorted 0;
      b = elemAt sorted 1;
    };
}
