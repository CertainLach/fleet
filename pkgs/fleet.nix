{ rustPlatform }:

rustPlatform.buildRustPackage rec {
  pname = "fleet";
  version = "0.0.1";
  name = "${pname}-${version}";

  src = ../.;
  cargoBuildFlags = "-p ${pname}";
  cargoLock = {
    lockFile = ../Cargo.lock;
  };
}
