{
  craneLib,
  installShellFiles,
}:
craneLib.buildPackage rec {
  pname = "fleet";

  src = craneLib.cleanCargoSource (craneLib.path ../.);
  strictDeps = true;

  cargoExtraArgs = "--locked -p ${pname}";

  nativeBuildInputs = [installShellFiles];

  postInstall = ''
    for shell in bash fish zsh; do
      installShellCompletion --cmd fleet \
        --$shell <($out/bin/fleet complete --shell $shell)
    done
  '';
}
