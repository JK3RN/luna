# Test report — LUNA v0.1.0

Date: 2026-11-22 · Source state: the tree of this release commit (identical file set; see `MANIFEST-0.1.0.txt`).

## Environment

| | |
| --- | --- |
| OS | Arch Linux (Omarchy), Linux 7.2.15, Hyprland (Wayland) |
| GPU | AMD Radeon Graphics (RADV RENOIR), Vulkan, Mesa 26.2.5 |
| Toolchain | rustc 1.93.1, cargo 1.93.1 |
| Data | fresh download 2026-11-22 via `scripts/fetch-data.sh` |

## Results

| # | Check | Result | Evidence |
| --- | --- | --- | --- |
| 1 | Fresh clone: `fetch-data.sh` downloads all 7 files; 6 pinned SHA-256 match | **PASS** | second run skips existing files |
| 2 | `cargo build --release --locked` from an empty target directory | **PASS** | 1 m 29 s |
| 3 | `luna-forge` prepares maps | **PASS** | 55 s; all four `.lmap` files bit-identical to the development build |
| 4 | `cargo test --release --locked` | **PASS** | 11 passed, 0 failed (incl. Horizons comparison and self-test) |
| 5 | `luna --verify` (data, computation chain, map orientation, image) | **PASS** | 49 checks passed, 0 failed |
| 6 | `cargo clippy --release --locked --all-targets` | **PASS** | 0 warnings |
| 7 | Invalid input (`--at 1800-01-01`, `--lat 95`, `nan`/`inf` values, `--size 0x0`, missing values) | **PASS** | clear message, exit code 2; covered by `falsche_eingaben_werden_klar_abgelehnt` |
| 8 | Window smoke test on Hyprland/Wayland: start, render, adaptive column, terminate | **PASS** | real `grim` screenshot (kept internal) |
| 9 | Headless renders for the showcase (`--render --hud`) | **PASS** | `docs/media/` |
| 10 | `luna --bench` 2560×1440, 3 scenes × 240 frames | **PASS** | worst p95 4.2–4.3 ms per frame over two runs (budget 16.7 ms) |
| 11 | Dependency advisories (OSV, 361 locked crates) | **PASS with notes** | no vulnerabilities; 2 "unmaintained" notices (`proc-macro-error2` via anise→tabled, build-time only; `ttf-parser` via winit→sctk-adwaita) |
| 12 | Uninstall = delete folder; nothing written outside project except requested PNGs | **PASS** | `--render`, `--info`, `--verify`: file scan of `$HOME` and `/tmp` found no new files besides the requested PNG (the GPU driver's own shader cache in `~/.cache` excepted) |
| 13 | `cargo fmt --check` | **SKIP** | project uses a wider line style without `rustfmt.toml`; not enforced |
| 14 | Other GPUs (Intel, NVIDIA), X11 session, other distributions | **SKIP** | not available in the test environment |
| 15 | Comparison with photographs, second computation path (Skyfield), glyph sharpness and idle load in the live window | **SKIP** | manual, not automated |

SKIP means *not tested*; it is not counted as passed.
