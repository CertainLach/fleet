{craneLib}:
craneLib.buildPackage rec {
  pname = "fleet";

  src = craneLib.cleanCargoSource (craneLib.path ../.);
  strictDeps = true;

  cargoExtraArgs = "--locked -p ${pname}";
}
