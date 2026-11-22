# Security Policy

LUNA is a local, offline desktop instrument. It opens no network sockets at runtime, runs no
privileged code and stores no credentials.

## Scope

- `luna` reads the data folder (`data/`), `~/.config/luna/config.toml`, the current Omarchy theme
  `colors.toml` (if present) and a system font found via `fc-match`. It writes only PNG files that
  the user requests (`--render`, `S` key → `~/Pictures/LUNA/`).
- All data files are verified against SHA-256 checksums in `data/manifest.json` on every start.
- `scripts/fetch-data.sh` downloads over HTTPS and checks pinned SHA-256 values for every immutable file.
- Saved PNGs contain the observer location in their metadata by design (receipt). Treat them as
  location-bearing files.

## Reporting

Please report vulnerabilities privately via GitHub's *Report a vulnerability* (Security Advisories)
for this repository rather than in a public issue. This is a hobby project; responses are best effort.

## Supported versions

Only the latest release receives fixes.
