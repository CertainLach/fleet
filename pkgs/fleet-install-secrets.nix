{ craneLib }:
craneLib.buildPackage rec {
  pname = "fleet-install-secrets";

  src = craneLib.cleanCargoSource (craneLib.path ../.);
  strictDeps = true;

  cargoExtraArgs = "--locked -p ${pname}";
}
