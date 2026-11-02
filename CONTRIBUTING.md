# Contributing to rumb (NixOS)

rumb shows the health of a NixOS system and only reads. Changes should keep it that way.

## Build and run

```sh
nix develop nixpkgs#cargo        # or any shell with cargo
cd tui && cargo build --release
RUMB_SCAN=../bin/rumb-scan target/release/rumb   # scans this system
target/release/rumb --demo                       # plays the bundled demo data
```

The flake at the root builds the package (`nix build .#rumb`) and the AppImage (`nix build .#appimage`).

## Checks

- in `tui/`: `cargo clippy --locked --release --all-targets -- -D warnings` and `cargo test --locked --release`
- in the repository root: `shellcheck --severity=warning bin/rumb-scan`

## Screen rules

- Every text is data, a legend or a key hint. No filler.
- A value that is not known yet stays empty. Nothing is guessed.
- Each fact appears once per screen. A detail window adds information instead of repeating what is visible.
- Colour is a signal only: red means act, yellow means look, the theme accent means problem. Everything else is neutral.
- Text keeps a contrast of at least 4.5:1 in every bundled theme.
- UI text is English, with accurate terms, units and plurals.

## Principles

- **Read-only.** rumb and `rumb-scan` never ask for root and never change the system: no `nixos-rebuild`, no garbage collection, no `nix flake update`. A test fails if the sources name sudo, pkexec, doas, su or run0, or if the scanner names a command that changes the system.
- **Optional parts.** niri, nvidia, Noctalia and network access are optional. Check that a tool exists before using it, and leave a value empty when it cannot be read.
- **Facts only.** Every number on screen comes from a command that rumb ran.

## Where things are

| Path | What |
|---|---|
| `bin/rumb-scan` | the scanner (bash): a stream of JSON events for the dashboard |
| `tui/src/app.rs` | the data from the scanner |
| `tui/src/page.rs` | layout and drawing |
| `tui/src/motion.rs` | animation, the gaze of the eye, the pupil text |
| `tui/src/ink.rs` | the pictures: eye, icons, graphs |
| `tui/src/pictures.rs`, `tui/src/sixel.rs` | image output for the terminal |
| `tui/src/export.rs` | the Markdown report (key `e`, and the output without a terminal) |
| `tui/src/theme.rs` | the bundled palettes and the live Noctalia colors |
| `tui/demo.ndjson` | the demo stream |
| `man/rumb.1` | the manual |

## Scanner changes

- `bin/rumb-scan` runs with `set -uo pipefail`: initialize variables, quote expansions and keep shellcheck clean.
- A missing or failing command must not stop the scan; report that check as failed instead.
- New data goes into the stream as new fields. The dashboard reads them with `#[serde(default)]`; add them to `tui/demo.ndjson` as well.

## License

Contributions are released under the [MIT License](LICENSE).
