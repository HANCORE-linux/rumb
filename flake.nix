{
  description = "rumb para NixOS: pacote nativo e AppImage";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    # Só o runtime type2 (estático) do nix-appimage é usado: o AppRun dele
    # monta um /nix próprio e esconderia as gerações do host.
    nix-appimage = {
      url = "github:ralismark/nix-appimage";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };

  outputs = { self, nixpkgs, nix-appimage }:
    let
      system = "x86_64-linux";
      pkgs = nixpkgs.legacyPackages.${system};
      static = pkgs.pkgsStatic;
      version = "0.1.9";

      scanTools = with pkgs; [ jq coreutils findutils gawk gnused gnugrep curl git nix systemd libxml2.bin ];

      common = {
        pname = "rumb";
        inherit version;
        src = ./.;
        cargoRoot = "tui";
        buildAndTestSubdir = "tui";
        cargoLock.lockFile = ./tui/Cargo.lock;
        meta = {
          description = "Terminal dashboard for NixOS system health";
          homepage = "https://github.com/HANCORE-linux/rumb";
          license = pkgs.lib.licenses.mit;
          mainProgram = "rumb";
        };
      };

      rumb = pkgs.rustPlatform.buildRustPackage (common // {
        nativeBuildInputs = [ pkgs.makeWrapper ];
        # Os testes rodam no checkPhase (cargo test) e precisam do scanner ao lado.
        postInstall = ''
          install -Dm755 bin/rumb-scan $out/libexec/rumb/rumb-scan
          substituteInPlace $out/libexec/rumb/rumb-scan \
            --replace-fail '#!/usr/bin/env bash' '#!${pkgs.bash}/bin/bash'
          install -Dm644 man/rumb.1 $out/share/man/man1/rumb.1
          wrapProgram $out/bin/rumb \
            --set RUMB_SCAN $out/libexec/rumb/rumb-scan \
            --suffix PATH : ${pkgs.lib.makeBinPath scanTools}
        '';
      });

      # Binário musl estático: roda em qualquer Linux sem /nix/store próprio.
      rumbStatic = static.rustPlatform.buildRustPackage (common // { doCheck = false; });

      runtime = nix-appimage.packages.${system}.appimage-runtimes.appimage-type2-runtime;

      # AppImage tradicional: o runtime só monta o squashfs e roda AppRun, sem
      # namespace, então o scanner enxerga /nix, /run/current-system e o journal
      # do host. bash, coreutils, nix, systemctl e curl vêm do NixOS; jq e
      # xmllint (que o NixOS não traz por padrão) vão junto, estáticos.
      appimage = pkgs.runCommand "rumb-${version}-x86_64.AppImage" { nativeBuildInputs = [ pkgs.squashfsTools ]; } ''
        app=AppDir
        mkdir -p $app/usr/bin $app/usr/libexec/rumb $app/usr/share/man/man1
        install -m755 ${rumbStatic}/bin/rumb $app/usr/bin/rumb
        install -m755 ${pkgs.lib.getBin static.jq}/bin/jq $app/usr/bin/jq
        install -m755 ${pkgs.lib.getBin static.libxml2}/bin/xmllint $app/usr/bin/xmllint
        install -m755 ${./bin/rumb-scan} $app/usr/libexec/rumb/rumb-scan
        install -m644 ${./man/rumb.1} $app/usr/share/man/man1/rumb.1
        cat > $app/AppRun <<'RUN'
        #!/bin/sh
        here=''${APPDIR:-$(dirname "$(readlink -f "$0")")}
        export RUMB_SCAN="$here/usr/libexec/rumb/rumb-scan"
        export PATH="$PATH:$here/usr/bin"
        exec "$here/usr/bin/rumb" "$@"
        RUN
        chmod 755 $app/AppRun
        cat > $app/rumb.desktop <<'DESKTOP'
        [Desktop Entry]
        Type=Application
        Name=rumb
        Comment=NixOS system health at a glance
        Exec=rumb
        Icon=rumb
        Terminal=true
        Categories=System;Monitor;
        DESKTOP
        cp ${./docs/logo-dark.png} $app/rumb.png
        ln -s rumb.png $app/.DirIcon
        mksquashfs $app rumb.squashfs -root-owned -noappend -comp zstd -quiet
        cat ${runtime} rumb.squashfs > $out
        chmod 755 $out
      '';
    in
    {
      packages.${system} = {
        inherit rumb rumbStatic appimage;
        default = rumb;
      };
      apps.${system}.default = { type = "app"; program = "${rumb}/bin/rumb"; };
    };
}
