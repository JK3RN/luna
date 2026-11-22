#!/usr/bin/env bash
# LUNA — Referenzwerte von JPL Horizons abrufen und einfrieren (nur während der Entwicklung).
# Die Teststandorte sind öffentliche Sternwarten, nicht der Standort des Nutzers.
# Die Tests selbst laufen danach ohne Netz gegen reference/horizons/*.txt.
set -euo pipefail
cd "$(dirname "$0")/../reference/horizons"

# 50 Zeitpunkte 2001–2026 (gemessene Erddrehung) + 8 außerhalb (Ausweichmodell).
TL_MAIN=$(python3 -I - <<'PY'
s = 20261115
out = []
for i in range(50):
    s = (s * 1103515245 + 12345) % 2**31
    out.append("'%.6f'" % (2451910.5 + (i + s / 2**31) * (9435.0 / 50)))
print(",".join(out))
PY
)
# 1905, 1935, 1955, 1968, 1975, 1988, 1997, 2031
TL_FALL="'2417000.271300','2427950.118800','2435300.904100','2440000.337700','2442600.662900','2447300.201400','2450600.845200','2462900.412600'"

fetch() { # name lon lat alt_km tlist
  curl -fsS -m 120 -G "https://ssd.jpl.nasa.gov/api/horizons.api" \
   --data-urlencode "format=text" --data-urlencode "COMMAND='301'" --data-urlencode "OBJ_DATA='NO'" \
   --data-urlencode "MAKE_EPHEM='YES'" --data-urlencode "EPHEM_TYPE='OBSERVER'" \
   --data-urlencode "CENTER='coord@399'" --data-urlencode "COORD_TYPE='GEODETIC'" \
   --data-urlencode "SITE_COORD='$2,$3,$4'" --data-urlencode "TLIST=$5" --data-urlencode "TLIST_TYPE='JD'" \
   --data-urlencode "TIME_TYPE='UT'" --data-urlencode "QUANTITIES='1,2,4,10,13,14,15,17,20,23,24,43'" \
   --data-urlencode "ANG_FORMAT='DEG'" --data-urlencode "CSV_FORMAT='YES'" --data-urlencode "EXTRA_PREC='YES'" \
   --data-urlencode "APPARENT='AIRLESS'" --data-urlencode "CAL_FORMAT='BOTH'" --data-urlencode "TIME_DIGITS='FRACSEC'" \
   --data-urlencode "RANGE_UNITS='KM'" --data-urlencode "REF_SYSTEM='ICRF'" > "$1.txt"
  grep -q '^\$\$SOE' "$1.txt" || { echo "Horizons-Antwort ohne Tabelle: $1" >&2; exit 1; }
  echo "ok $1: $(sed -n '/^\$\$SOE/,/^\$\$EOE/p' "$1.txt" | grep -c ',') Zeilen"
}

for kind in main fall; do
  [ $kind = main ] && TL="$TL_MAIN" || TL="$TL_FALL"
  fetch "greenwich_$kind"  0.0      51.4769  0.046 "$TL"; sleep 1
  fetch "maunakea_$kind"   204.5319 19.8207  4.205 "$TL"; sleep 1
  fetch "sydney_$kind"     151.2045 -33.8597 0.040 "$TL"; sleep 1
done
