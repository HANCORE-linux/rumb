<h1 align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="docs/logo-dark.png">
    <img src="docs/logo-light.png" alt="rumb" width="320">
  </picture>
</h1>

<p align="center">NixOS system health at a glance.</p>

<p align="center">A terminal dashboard for NixOS: which flake inputs are behind, what is failing since the last switch, which generations you can roll back to, and how much room is left on <code>/nix</code> and <code>/boot</code>. It only reads and never asks for root.</p>

This is a NixOS fork of [HANCORE-linux/rumb](https://github.com/HANCORE-linux/rumb). The look, the eye and the dashboard come from upstream; the scanner and the instruments were rewritten for NixOS.

## Run

From this repository:

```sh
nix run github:jardel20/rumb/nixos            # the dashboard
nix run github:jardel20/rumb/nixos -- --demo  # bundled demo data
nix build .#appimage                          # an AppImage (inside a clone)
```

Or add this flake as an input and put `packages.x86_64-linux.default` in `environment.systemPackages`.

## What it shows

| Instrument | Source |
|---|---|
| PENDING | flake inputs of `/etc/nixos/flake.lock` (or `$RUMB_FLAKE`) whose remote moved on, ranked by how old the lock is |
| PROBLEMS / DAY | core dumps, journal errors and the days the system switched generation |
| SINCE SWITCH | how long the current generation runs and which packages it changed (`nix store diff-closures`) |
| GENERATIONS | the current generation, how many are kept, the rollback target, the oldest |
| DISK AND STORE | `/nix` use, store paths of the system closure, system and user packages, old generations, closure size, `/boot` free |
| BEFORE YOU UPDATE | risk, reboot (booted vs current kernel, initrd and modules), programs running replaced files, rollback, `/boot`, space, NixOS news |

The subsystems around the eye group updates and problems by kernel, graphics, compositor (niri), toolkit, shell (Noctalia), base and other.

## Terminals

rumb draws the eye, the icons and the graphs as images. foot, kitty and Ghostty are supported; other terminals show the text version.

It starts with the instruments and the eye (120×34 or larger); keys 3, 4 and 5 add the updates, problems and live events lists. A smaller terminal hides the eye first, then the lists that do not fit, then the instruments.

## Themes

The default theme is `shadesofjade`; `esc` opens the options in the eye to pick another, and rumb remembers it. `--themes` lists the bundled palettes. The `system` theme follows the colors Noctalia writes to `~/.config/noctalia/colors.json`.

[MIT](LICENSE)
