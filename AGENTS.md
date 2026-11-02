# AGENTS.md

Rules for AI agents working on this NixOS fork of rumb.

- rumb only reads. The scanner never runs or names sudo, pkexec, doas, su or run0, and never changes the system: no `nixos-rebuild`, `nix-collect-garbage`, `nix-env`, `nix flake update` or `nix store gc`. `tui/tests/no_privilege_escalation.rs` enforces this.
- Ask before committing or pushing.
- Keep the look of upstream rumb: layout, colours, the eye and its motion. Visual changes are verified on the real binary (`rumb --demo`, or a recorded stream through `RUMB_SCAN`), at several terminal sizes.
- Every text on screen is data, a legend or a key hint; a value that is not known stays empty; each fact appears once per screen; colour is a signal only.
- The scanner and the dashboard talk through the event stream. Read new stream fields with `#[serde(default)]` and show them in `tui/demo.ndjson`.
- Keep heavy work such as painting and encoding pictures off the UI thread.
- Before asking to commit, run `cargo clippy --locked --release --all-targets -- -D warnings`, `cargo test --locked --release` in `tui/` and `shellcheck --severity=warning bin/rumb-scan`. Say what you did not check.

| Variable | Use |
|---|---|
| `RUMB_SCAN` | path of the scanner to run, for example a fake one for tests |
| `RUMB_FLAKE` | directory of the system flake (default `/etc/nixos`) |
| `RUMB_THEME` | theme name, like `--theme` |
| `RUMB_EVENTS` | a recorded stream for the frame tests instead of the demo |
| `RUMB_DUMP` | output directory for the `mockup_frames` test |
| `RUMB_VIDEO` | output directory for the `motion_frames` test, with `RUMB_ONLY`, `RUMB_SECONDS`, `RUMB_COLUMNS`, `RUMB_ROWS`, `RUMB_CELL_WIDTH`, `RUMB_CELL_HEIGHT`, `RUMB_SELECT`, `RUMB_PANELS`, `RUMB_KEYS` and `RUMB_NO_PICTURES` |
