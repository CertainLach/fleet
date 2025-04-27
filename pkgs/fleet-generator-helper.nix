{ craneLib }:
craneLib.buildPackage rec {
  pname = "fleet-generator-helper";

  src = craneLib.cleanCargoSource (craneLib.path ../.);
  strictDeps = true;

  cargoExtraArgs = "--locked -p ${pname}";

  postInstall = ''
    ln -s $out/bin/${pname} $out/bin/gh
  '';
}
