# Shared functions for fleet configuration, available as `fleet` module argument
{ nixpkgs, hosts }: with nixpkgs.lib; rec {
  mkSecret = let
    system = builtins.currentSystem;
    pkgs = import nixpkgs { inherit system; };
    keys = builtins.getEnv "RAGE_KEYS";
    encryptCmd = "rage ${keys} -a";
    impuritySource = builtins.getEnv "IMPURITY_SOURCE";
  in
    f: let
      data = f { inherit pkgs encryptCmd; };
    in
      builtins.derivation {
        inherit system;
        name = "secret";

        builder = "${pkgs.bash}/bin/bash";
        args = [
          (
            pkgs.writeTextFile {
              name = "./build-${impuritySource}.sh";
              text = data.script;
              executable = true;
            }
          )
        ];

        PATH = "${pkgs.coreutils}/bin:${pkgs.rage}/bin${builtins.concatStringsSep "" (builtins.map (n: ":${n}/bin") data.utils)}";
      };
  # Modules can't register hosts because of infinite recursion
  hostNames = attrNames hosts;
  hostsToAttrs = f: listToAttrs (
    map (name: { inherit name; value = f name; }) hostNames
  );
  hostsCartesian = remove null (
    unique (
      crossLists (
        a: b: if a == b then
          null
        else
          hostsPair a b
      ) [ hostNames hostNames ]
    )
  );
  hostsPair = this: other: let
    sorted = sort (a: b: a < b) [ this other ];
  in
    {
      a = elemAt sorted 0;
      b = elemAt sorted 1;
    };
}
