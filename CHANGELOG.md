# Changelog

## v0.1.0 — Erstes Licht (pre-release)

First public source release of the Stage 0 prototype.

- Ephemeris chain on JPL DE440 via ANISE: topocentric position, illumination, phase angle, distance,
  diameter, libration, axis position angle, altitude/azimuth; Earth-rotation fallback outside the
  measured series.
- GPU renderer (wgpu/Vulkan): LOLA slopes, eight-direction horizon maps for cast shadows, penumbra,
  Lommel-Seeliger scattering fitted to Allen's lunar phase curve, EV-based exposure.
- Window with adaptive instrument column, provenance tags (M/B/N/K), time-lapse, free rotation,
  Omarchy theme colours; headless `--render`, `--info`, `--bench`, `--verify`.
- `luna-forge` data preparation and `scripts/fetch-data.sh` with pinned SHA-256 checksums.
- Self-test against frozen JPL Horizons values for three public observatories.
- Release hardening: non-finite numbers (`nan`, `inf`) in command-line options, the config file and
  `--at JD…` are now rejected with a clear message instead of panicking or rendering an empty image.
