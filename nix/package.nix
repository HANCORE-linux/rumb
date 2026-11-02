# O pacote nativo. Recebe o pkgs de quem instala (flake, overlay ou módulo),
# então usa o nixpkgs do próprio sistema.
{ lib
, rustPlatform
, makeWrapper
, bash
, jq
, coreutils
, findutils
, gawk
, gnused
, gnugrep
, curl
, git
, nix
, systemd
, libxml2
}:

rustPlatform.buildRustPackage {
  pname = "rumb";
  version = "0.1.9";

  src = lib.cleanSource ../.;
  cargoRoot = "tui";
  buildAndTestSubdir = "tui";
  cargoLock.lockFile = ../tui/Cargo.lock;

  nativeBuildInputs = [ makeWrapper ];

  # Os testes rodam no checkPhase (cargo test) e precisam do scanner ao lado.
  postInstall = ''
    install -Dm755 bin/rumb-scan $out/libexec/rumb/rumb-scan
    substituteInPlace $out/libexec/rumb/rumb-scan \
      --replace-fail '#!/usr/bin/env bash' '#!${bash}/bin/bash'
    install -Dm644 man/rumb.1 $out/share/man/man1/rumb.1
    wrapProgram $out/bin/rumb \
      --set RUMB_SCAN $out/libexec/rumb/rumb-scan \
      --suffix PATH : ${lib.makeBinPath [ jq coreutils findutils gawk gnused gnugrep curl git nix systemd libxml2.bin ]}
  '';

  meta = {
    description = "Terminal dashboard for NixOS system health";
    homepage = "https://github.com/jardel20/rumb";
    license = lib.licenses.mit;
    mainProgram = "rumb";
    platforms = lib.platforms.linux;
  };
}
