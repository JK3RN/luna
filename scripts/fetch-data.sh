#!/usr/bin/env bash
# LUNA — einmaliger Abruf der Originaldaten von NASA und JPL.
# Lädt nach data/raw/ und schreibt data/raw/ABRUF.tsv (Datei, Quelle, Abrufdatum, SHA-256).
# Bereits vorhandene Dateien werden nicht erneut geladen.
set -euo pipefail

cd "$(dirname "$0")/.."
RAW=data/raw
mkdir -p "$RAW"

SVS=https://svs.gsfc.nasa.gov/vis/a000000/a004700/a004720
NAIF=https://naif.jpl.nasa.gov/pub/naif/generic_kernels

FILES=(
  "ldem_16_uint.tif|$SVS/ldem_16_uint.tif"
  "lroc_color_16bit_srgb_8k.tif|$SVS/lroc_color_16bit_srgb_8k.tif"
  "de440s.bsp|https://ssd.jpl.nasa.gov/ftp/eph/planets/bsp/de440s.bsp"
  "moon_pa_de440_200625.bpc|$NAIF/pck/moon_pa_de440_200625.bpc"
  "earth_latest_high_prec.bpc|$NAIF/pck/earth_latest_high_prec.bpc"
  "pck00011.tpc|$NAIF/pck/pck00011.tpc"
  "moon_de440_250416.tf|$NAIF/fk/satellites/moon_de440_250416.tf"
)

# Festgeschriebene SHA-256 der unveränderlichen Dateien (Stand des ersten Abrufs, 2026-11-15).
# earth_latest_high_prec.bpc wird von JPL täglich fortgeschrieben und hat deshalb keinen festen Wert;
# sein Stand wird beim Abruf im Protokoll und danach im Manifest festgehalten.
declare -A PINNED=(
  [ldem_16_uint.tif]=45a2b32d56e81ed30db07fead8abc842b249b6511219d9ca2c53f81bc2dc5d62
  [lroc_color_16bit_srgb_8k.tif]=db7808e878b6a55eb409bb231eab8deb477f84b5c9d7396d76ff73e5d54992d9
  [de440s.bsp]=c1c7feeab882263fc493a9d5a5b2ddd71b54826cdf65d8d17a76126b260a49f2
  [moon_pa_de440_200625.bpc]=60cd55aa401ea2ea97360636f567554bfe4e37bb829f901b4460a455dfaf783f
  [pck00011.tpc]=3dff7b1dbeceaa01f25467767d3fa25816051c85d162d1edf04acb310ee28bb1
  [moon_de440_250416.tf]=a47c71e9c9f33796bdafb2c9d69a7ee447b6016ecad80f71cd6f3e479f9cf768
)

LOG="$RAW/ABRUF.tsv"
[ -f "$LOG" ] || printf 'datei\tquelle\tabruf_utc\tsha256\tbytes\n' > "$LOG"

for entry in "${FILES[@]}"; do
  name="${entry%%|*}"
  url="${entry#*|}"
  if [ -s "$RAW/$name" ]; then
    echo "vorhanden  $name"
    continue
  fi
  echo "lade       $name"
  curl -fL --retry 3 --retry-delay 5 -sS -o "$RAW/$name.part" "$url"
  sum=$(sha256sum "$RAW/$name.part" | cut -d' ' -f1)
  want="${PINNED[$name]:-}"
  if [ -n "$want" ] && [ "$sum" != "$want" ]; then
    echo "FEHLER: $name hat eine andere Prüfsumme als festgeschrieben." >&2
    echo "  erwartet $want" >&2
    echo "  erhalten $sum" >&2
    echo "  Die Quelle hat die Datei geändert oder der Abruf ist beschädigt. Datei bleibt als $name.part liegen." >&2
    exit 1
  fi
  mv "$RAW/$name.part" "$RAW/$name"
  size=$(stat -c %s "$RAW/$name")
  printf '%s\t%s\t%s\t%s\t%s\n' "$name" "$url" "$(date -u +%Y-%m-%dT%H:%M:%SZ)" "$sum" "$size" >> "$LOG"
done
echo "fertig."
