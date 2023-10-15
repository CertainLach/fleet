{ rustPlatform, lib }:

rustPlatform.buildRustPackage rec {
  pname = "fleet-install-secrets";
  version = "0.0.1";
  name = "${pname}-${version}";

  src = ../.;
  buildAndTestSubdir = "cmds/install-secrets";
  cargoLock = {
    lockFile = ../Cargo.lock;
    outputHashes = {
      "alejandra-3.0.0" = "sha256-lStDIPizbJipd1JpNKX1olBKzyIosyC2U/mVFwJPcZE=";
    };
  };
}
