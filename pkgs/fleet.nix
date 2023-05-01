{ rustPlatform }:

rustPlatform.buildRustPackage rec {
  pname = "fleet";
  version = "0.0.1";
  name = "${pname}-${version}";

  src = ../.;
  cargoBuildFlags = "-p ${pname}";
  cargoLock = {
    lockFile = ../Cargo.lock;
    outputHashes = {
      "alejandra-3.0.0" = "sha256-YSdHsJ73G7TEFzbmpZ2peuMefIa9/vNB2g+xdiyma3U=";
    };
  };
}
