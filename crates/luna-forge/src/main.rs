//! luna-forge — bereitet die Originaldateien von NASA einmalig für LUNA auf.
//!
//! Eingabe (data/raw):  ldem_16_uint.tif (LOLA-Höhen), lroc_color_16bit_srgb_8k.tif (Farbkarte)
//! Ausgabe (data/forged): albedo.lmap, relief.lmap, horizon_a.lmap, horizon_b.lmap
//! sowie data/manifest.json mit Quellen, Prüfsummen und Verarbeitungsschritten.

use std::f64::consts::PI;
use std::fs::File;
use std::io::BufReader;
use std::path::{Path, PathBuf};
use std::time::Instant;

use anyhow::{anyhow, bail, Context, Result};
use luna_data::lmap::{self, LmapFormat};
use luna_data::manifest::sha256_file;
use luna_data::{DataPaths, ForgeInfo, Manifest, ManifestEntry};
use rayon::prelude::*;
use tiff::decoder::{Decoder, DecodingResult, Limits};

const MOON_RADIUS_KM: f64 = 1737.4;
/// Die 16-Bit-Höhenkarte zählt halbe Meter ab einem Radius von 1 727 400 m (Angabe der Quelle).
const HEIGHT_BASE_M: f64 = 1_727_400.0 - 1_737_400.0;
const HEIGHT_UNIT_M: f64 = 0.5;
const HORIZON_MAX_ANGLE_DEG: f64 = 45.0;
const HORIZON_DIRS: usize = 8;
/// Schrittweiten der Horizontsuche in Äquatortexeln: erst dicht, dann geometrisch wachsend.
const NEAR_STEPS: usize = 24;
const FAR_GROWTH: f64 = 1.12;
const FAR_LIMIT: f64 = 140.0;

fn read_tiff_u16(path: &Path) -> Result<(u32, u32, usize, Vec<u16>)> {
    let f = File::open(path).with_context(|| format!("{} fehlt — bitte scripts/fetch-data.sh ausführen", path.display()))?;
    let mut dec = Decoder::new(BufReader::with_capacity(1 << 20, f))?.with_limits(Limits::unlimited());
    let (w, h) = dec.dimensions()?;
    let data = match dec.read_image()? {
        DecodingResult::U16(v) => v,
        _ => bail!("{}: erwartet 16 Bit ohne Vorzeichen", path.display()),
    };
    let channels = data.len() / (w as usize * h as usize);
    if channels * w as usize * h as usize != data.len() {
        bail!("{}: Datenmenge passt nicht zur Bildgröße", path.display());
    }
    Ok((w, h, channels, data))
}

/// Höhenkarte in Metern über der Bezugskugel, mit Zugriff in Plattkartenkoordinaten.
struct Height {
    w: usize,
    h: usize,
    m: Vec<f32>,
}

impl Height {
    /// Bilineare Abtastung an stetigen Texelkoordinaten (Texelmitten bei ganzzahligen Werten).
    #[inline]
    fn sample(&self, x: f64, y: f64) -> f64 {
        let y = y.clamp(0.0, (self.h - 1) as f64);
        let x0 = x.floor();
        let y0 = y.floor();
        let (fx, fy) = (x - x0, y - y0);
        let xi = (x0 as i64).rem_euclid(self.w as i64) as usize;
        let xj = if xi + 1 == self.w { 0 } else { xi + 1 };
        let yi = y0 as usize;
        let yj = (yi + 1).min(self.h - 1);
        let a = self.m[yi * self.w + xi] as f64;
        let b = self.m[yi * self.w + xj] as f64;
        let c = self.m[yj * self.w + xi] as f64;
        let d = self.m[yj * self.w + xj] as f64;
        (a * (1.0 - fx) + b * fx) * (1.0 - fy) + (c * (1.0 - fx) + d * fx) * fy
    }
    #[inline]
    fn at(&self, x: i64, y: i64) -> f64 {
        let xi = x.rem_euclid(self.w as i64) as usize;
        let yi = y.clamp(0, self.h as i64 - 1) as usize;
        self.m[yi * self.w + xi] as f64
    }
    /// Länge/Breite (Grad) der Texelmitte.
    fn lon_lat(&self, x: usize, y: usize) -> (f64, f64) {
        (
            -180.0 + (x as f64 + 0.5) * 360.0 / self.w as f64,
            90.0 - (y as f64 + 0.5) * 180.0 / self.h as f64,
        )
    }
    /// Stetige Texelkoordinaten zu Länge/Breite (Grad).
    #[inline]
    fn xy(&self, lon: f64, lat: f64) -> (f64, f64) {
        ((lon + 180.0) * self.w as f64 / 360.0 - 0.5, (90.0 - lat) * self.h as f64 / 180.0 - 0.5)
    }
    fn downsample(&self, f: usize) -> Height {
        let (w, h) = (self.w / f, self.h / f);
        let mut m = vec![0f32; w * h];
        m.par_chunks_mut(w).enumerate().for_each(|(y, row)| {
            for (x, out) in row.iter_mut().enumerate() {
                let mut s = 0.0f64;
                for dy in 0..f {
                    for dx in 0..f {
                        s += self.m[(y * f + dy) * self.w + x * f + dx] as f64;
                    }
                }
                *out = (s / (f * f) as f64) as f32;
            }
        });
        Height { w, h, m }
    }
}

fn srgb_to_linear(v: f64) -> f64 {
    if v <= 0.04045 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) }
}
fn linear_to_srgb(v: f64) -> f64 {
    if v <= 0.003_130_8 { v * 12.92 } else { 1.055 * v.powf(1.0 / 2.4) - 0.055 }
}

/// Halbiert eine 4-Kanal-8-Bit-Stufe. `srgb`: Mittelung im linearen Licht.
fn halve_rgba8(src: &[u8], w: usize, h: usize, srgb: bool) -> Vec<u8> {
    let (nw, nh) = (w / 2, h / 2);
    let lut: Vec<f32> = (0..256).map(|i| if srgb { srgb_to_linear(i as f64 / 255.0) as f32 } else { i as f32 / 255.0 }).collect();
    let mut out = vec![0u8; nw * nh * 4];
    out.par_chunks_mut(nw * 4).enumerate().for_each(|(y, row)| {
        for x in 0..nw {
            for c in 0..4 {
                let mut s = 0.0f32;
                for (dy, dx) in [(0, 0), (0, 1), (1, 0), (1, 1)] {
                    s += lut[src[((y * 2 + dy) * w + x * 2 + dx) * 4 + c] as usize];
                }
                let v = (s * 0.25) as f64;
                let e = if srgb && c < 3 { linear_to_srgb(v) } else { v };
                row[x * 4 + c] = (e * 255.0).round().clamp(0.0, 255.0) as u8;
            }
        }
    });
    out
}

fn halve_rg16f(src: &[u8], w: usize, h: usize) -> Vec<u8> {
    let (nw, nh) = (w / 2, h / 2);
    let get = |i: usize| half::f16::from_le_bytes([src[i * 2], src[i * 2 + 1]]).to_f32();
    let mut out = vec![0u8; nw * nh * 4];
    out.par_chunks_mut(nw * 4).enumerate().for_each(|(y, row)| {
        for x in 0..nw {
            for c in 0..2 {
                let mut s = 0.0f32;
                for (dy, dx) in [(0, 0), (0, 1), (1, 0), (1, 1)] {
                    s += get(((y * 2 + dy) * w + x * 2 + dx) * 2 + c);
                }
                let b = half::f16::from_f32(s * 0.25).to_le_bytes();
                row[x * 4 + c * 2] = b[0];
                row[x * 4 + c * 2 + 1] = b[1];
            }
        }
    });
    out
}

fn build_levels(base: Vec<u8>, w: u32, h: u32, f: impl Fn(&[u8], usize, usize) -> Vec<u8>) -> Vec<Vec<u8>> {
    let n = lmap::mip_count(w, h);
    let mut levels = vec![base];
    for l in 1..n {
        let (pw, ph) = lmap::level_size(w, h, l - 1);
        let next = f(levels.last().unwrap(), pw as usize, ph as usize);
        levels.push(next);
    }
    levels
}

/// Neigungen der Oberfläche: (dh/dOst, dh/dNord), dimensionslos.
fn slopes(hm: &Height) -> Vec<u8> {
    let dlat_km = MOON_RADIUS_KM * PI / hm.h as f64;
    let mut out = vec![0u8; hm.w * hm.h * 4];
    out.par_chunks_mut(hm.w * 4).enumerate().for_each(|(y, row)| {
        let (_, lat) = hm.lon_lat(0, y);
        let cosl = (lat * PI / 180.0).cos();
        // Zu den Polen hin schrumpfen die Texel in Ost-West-Richtung. Damit die Ableitung
        // nicht über wenige Meter gebildet wird, wächst der Abstand der Stützstellen mit.
        let k = ((1.0 / cosl).round() as i64).clamp(1, hm.w as i64 / 4);
        let dx_km = 2.0 * k as f64 * dlat_km * cosl;
        for x in 0..hm.w {
            let (xi, yi) = (x as i64, y as i64);
            let de = (hm.at(xi + k, yi) - hm.at(xi - k, yi)) / 1000.0 / dx_km;
            let (yn, ys) = ((yi - 1).max(0), (yi + 1).min(hm.h as i64 - 1));
            let dn = (hm.at(xi, yn) - hm.at(xi, ys)) / 1000.0 / ((ys - yn) as f64 * dlat_km);
            let e = half::f16::from_f32(de as f32).to_le_bytes();
            let n = half::f16::from_f32(dn as f32).to_le_bytes();
            row[x * 4..x * 4 + 4].copy_from_slice(&[e[0], e[1], n[0], n[1]]);
        }
    });
    out
}

/// Horizontwinkel je Texel in acht Himmelsrichtungen (N, NO, O, SO, S, SW, W, NW).
///
/// Für jeden Punkt läuft die Suche entlang eines Großkreises und merkt sich den größten
/// Höhenwinkel des Geländes über der örtlichen Waagerechten; die Krümmung der Kugel ist
/// berücksichtigt. Ergebnis: zwei 4-Kanal-Karten, Winkel als sign(a)·sqrt(|a|/max) kodiert.
fn horizons(hm: &Height) -> (Vec<u8>, Vec<u8>, f64) {
    let coarse = hm.downsample(4);
    let texel_rad = PI / hm.h as f64;
    let mut steps: Vec<f64> = (1..=NEAR_STEPS).map(|i| i as f64).collect();
    let mut d = NEAR_STEPS as f64;
    while d * FAR_GROWTH <= FAR_LIMIT {
        d *= FAR_GROWTH;
        steps.push(d);
    }
    let reach_km = steps.last().unwrap() * texel_rad * MOON_RADIUS_KM;
    let trig: Vec<(f64, f64, bool)> = steps.iter().map(|&s| ((s * texel_rad).sin(), (s * texel_rad).cos(), s > NEAR_STEPS as f64)).collect();
    let az: Vec<(f64, f64)> = (0..HORIZON_DIRS).map(|i| (i as f64 * 2.0 * PI / HORIZON_DIRS as f64).sin_cos()).collect();
    let max_a = HORIZON_MAX_ANGLE_DEG * PI / 180.0;

    let mut a = vec![0u8; hm.w * hm.h * 4];
    let mut b = vec![0u8; hm.w * hm.h * 4];
    a.par_chunks_mut(hm.w * 4).zip(b.par_chunks_mut(hm.w * 4)).enumerate().for_each(|(y, (ra, rb))| {
        for x in 0..hm.w {
            let (lon, lat) = hm.lon_lat(x, y);
            let (sl, cl) = (lon * PI / 180.0).sin_cos();
            let (sp, cp) = (lat * PI / 180.0).sin_cos();
            let p = [cp * cl, cp * sl, sp];
            let east = [-sl, cl, 0.0];
            let north = [-sp * cl, -sp * sl, cp];
            let r0 = MOON_RADIUS_KM + hm.m[y * hm.w + x] as f64 / 1000.0;
            for (dir, &(sa, ca)) in az.iter().enumerate() {
                let t = [north[0] * ca + east[0] * sa, north[1] * ca + east[1] * sa, north[2] * ca + east[2] * sa];
                let mut best = f64::NEG_INFINITY;
                for &(st, ct, far) in &trig {
                    let q = [p[0] * ct + t[0] * st, p[1] * ct + t[1] * st, p[2] * ct + t[2] * st];
                    let qlat = q[2].clamp(-1.0, 1.0).asin() * 180.0 / PI;
                    let qlon = q[1].atan2(q[0]) * 180.0 / PI;
                    let h1 = if far {
                        let (cx, cy) = coarse.xy(qlon, qlat);
                        coarse.sample(cx, cy)
                    } else {
                        let (fx, fy) = hm.xy(qlon, qlat);
                        hm.sample(fx, fy)
                    };
                    let r1 = MOON_RADIUS_KM + h1 / 1000.0;
                    let tan_e = (r1 * ct - r0) / (r1 * st);
                    if tan_e > best {
                        best = tan_e;
                    }
                }
                let ang = best.atan().clamp(-max_a, max_a);
                let enc = 0.5 + 0.5 * ang.signum() * (ang.abs() / max_a).sqrt();
                let v = (enc * 255.0).round().clamp(0.0, 255.0) as u8;
                if dir < 4 {
                    ra[x * 4 + dir] = v;
                } else {
                    rb[x * 4 + dir - 4] = v;
                }
            }
        }
    });
    (a, b, reach_km)
}

/// Mittlere Höhe in einer Kreisscheibe bzw. einem Ring um einen Punkt (km-Radien).
fn ring_mean(hm: &Height, lon: f64, lat: f64, r_in_km: f64, r_out_km: f64) -> f64 {
    let (mut s, mut n) = (0.0, 0.0);
    let steps = 24;
    for ir in 0..=steps {
        let r = r_in_km + (r_out_km - r_in_km) * ir as f64 / steps as f64;
        for ia in 0..72 {
            let a = ia as f64 * 5.0 * PI / 180.0;
            let dlat = r * a.cos() / MOON_RADIUS_KM * 180.0 / PI;
            let dlon = r * a.sin() / (MOON_RADIUS_KM * (lat * PI / 180.0).cos()) * 180.0 / PI;
            let (x, y) = hm.xy(lon + dlon, lat + dlat);
            s += hm.sample(x, y);
            n += 1.0;
        }
    }
    s / n
}

fn albedo_lum(rgba: &[u8], w: usize, h: usize, lon: f64, lat: f64, r_deg: f64) -> f64 {
    let (mut s, mut n) = (0.0, 0.0);
    let r_px = (r_deg * w as f64 / 360.0).ceil() as i64;
    let cx = ((lon + 180.0) * w as f64 / 360.0) as i64;
    let cy = ((90.0 - lat) * h as f64 / 180.0) as i64;
    for dy in -r_px..=r_px {
        for dx in -r_px..=r_px {
            let x = (cx + dx).rem_euclid(w as i64) as usize;
            let y = (cy + dy).clamp(0, h as i64 - 1) as usize;
            let i = (y * w + x) * 4;
            s += 0.2126 * srgb_to_linear(rgba[i] as f64 / 255.0)
                + 0.7152 * srgb_to_linear(rgba[i + 1] as f64 / 255.0)
                + 0.0722 * srgb_to_linear(rgba[i + 2] as f64 / 255.0);
            n += 1.0;
        }
    }
    s / n
}

fn now_utc() -> String {
    let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    let days = secs / 86400;
    let rem = secs % 86400;
    // Tage seit 1970-01-01 → Kalenderdatum (Algorithmus nach Howard Hinnant).
    let z = days as i64 + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + if m <= 2 { 1 } else { 0 };
    format!("{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z", rem / 3600, rem % 3600 / 60, rem % 60)
}

fn raw_entries(paths: &DataPaths) -> Result<Vec<ManifestEntry>> {
    let log = paths.raw().join("ABRUF.tsv");
    let text = std::fs::read_to_string(&log).with_context(|| format!("{} fehlt — bitte scripts/fetch-data.sh ausführen", log.display()))?;
    let product = |name: &str| -> &'static str {
        match name {
            "ldem_16_uint.tif" => "CGI Moon Kit, LOLA-Höhenkarte 16 Pixel/Grad — NASA's Scientific Visualization Studio; LRO/LOLA",
            "lroc_color_16bit_srgb_8k.tif" => "CGI Moon Kit, LROC-WAC-Farbkarte 8k (Fassung 2025, für Ästhetik optimiert) — NASA's Scientific Visualization Studio; LRO/LROC",
            "de440s.bsp" => "JPL Planetary and Lunar Ephemeris DE440, Kurzfassung — NASA/JPL; Park et al. 2021",
            "moon_pa_de440_200625.bpc" => "Drehlage des Mondes, Hauptachsensystem DE440 — NASA/JPL NAIF",
            "earth_latest_high_prec.bpc" => "Erddrehung ITRF93, gemessen und vorhergesagt — NASA/JPL NAIF",
            "pck00011.tpc" => "Körperkonstanten (IAU 2015) — NASA/JPL NAIF",
            "moon_de440_250416.tf" => "Bezugssysteme des Mondes zu DE440 — NASA/JPL NAIF",
            _ => "unbekannt",
        }
    };
    let mut out = Vec::new();
    for line in text.lines().skip(1) {
        let c: Vec<&str> = line.split('\t').collect();
        if c.len() < 5 {
            continue;
        }
        let rel = format!("raw/{}", c[0]);
        let (sum, bytes) = sha256_file(&paths.root.join(&rel))?;
        if sum != c[3] {
            bail!("{}: Prüfsumme weicht vom Abrufprotokoll ab", c[0]);
        }
        out.push(ManifestEntry {
            path: rel,
            sha256: sum,
            bytes,
            source: c[1].to_string(),
            product: product(c[0]).to_string(),
            retrieved_utc: c[2].to_string(),
            steps: vec!["unverändert von der Quelle geladen".to_string()],
        });
    }
    if out.len() < 7 {
        bail!("Abrufprotokoll unvollständig ({} von 7 Dateien)", out.len());
    }
    Ok(out)
}

fn main() -> Result<()> {
    let mut root: Option<PathBuf> = None;
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--data" => root = Some(PathBuf::from(args.next().ok_or_else(|| anyhow!("--data braucht einen Pfad"))?)),
            "-h" | "--help" => {
                println!("luna-forge [--data <Datenordner>]\n\nBereitet die Originaldateien in data/raw einmalig auf und schreibt data/forged und data/manifest.json.");
                return Ok(());
            }
            other => bail!("unbekannter Schalter '{other}' (siehe --help)"),
        }
    }
    let paths = match root {
        Some(r) => DataPaths { root: r },
        None => DataPaths::locate()?,
    };
    std::fs::create_dir_all(paths.forged())?;
    let t0 = Instant::now();
    let mut checks = Vec::new();

    // --- Höhen ---
    println!("lese Höhenkarte …");
    let (w, h, ch, raw) = read_tiff_u16(&paths.raw().join("ldem_16_uint.tif"))?;
    if ch != 1 || w != 2 * h {
        bail!("Höhenkarte: unerwartetes Format {w}×{h}, {ch} Kanäle");
    }
    let hm = Height {
        w: w as usize,
        h: h as usize,
        m: raw.par_iter().map(|&v| (HEIGHT_BASE_M + v as f64 * HEIGHT_UNIT_M) as f32).collect(),
    };
    drop(raw);
    let (mut imin, mut imax) = (0usize, 0usize);
    for (i, &v) in hm.m.iter().enumerate() {
        if v < hm.m[imin] {
            imin = i;
        }
        if v > hm.m[imax] {
            imax = i;
        }
    }
    let (hmin, hmax) = (hm.m[imin] as f64, hm.m[imax] as f64);
    let (lon_max, lat_max) = hm.lon_lat(imax % hm.w, imax / hm.w);
    let (lon_min, lat_min) = hm.lon_lat(imin % hm.w, imin / hm.w);
    println!("  {}×{}, Höhen {:.0} m bis {:.0} m", w, h, hmin, hmax);

    // Kartenlage: Der höchste Punkt des Mondes liegt laut LOLA bei 5,4125° N, 201,3665° O (10 786 m).
    let (ref_lon, ref_lat) = (201.3665 - 360.0, 5.4125);
    let texel_deg = 360.0 / w as f64;
    let off = ((lon_max - ref_lon).powi(2) + (lat_max - ref_lat).powi(2)).sqrt() / texel_deg;
    let ok = off <= 1.5;
    checks.push(format!(
        "{}: höchster Punkt der Karte bei {:.4}° N, {:.4}° O ({:.0} m); LOLA-Angabe 5,4125° N, 201,3665° O (10 786 m); Abstand {:.2} Texel",
        if ok { "besteht" } else { "VERFEHLT" },
        lat_max,
        lon_max.rem_euclid(360.0),
        hmax,
        off
    ));
    checks.push(format!("Hinweis: tiefster Punkt der Karte bei {:.3}° N, {:.3}° O ({:.0} m)", lat_min, lon_min.rem_euclid(360.0), hmin));
    // Krater: Boden muss tiefer liegen als der Rand — am Ort selbst, nicht am Ost-West-Spiegelbild.
    for (name, lat, lon, diam_km) in [("Tycho", -43.31, -11.36, 85.0), ("Copernicus", 9.62, -20.08, 93.0), ("Tsiolkovskiy", -20.4, 129.1, 185.0)] {
        let r = diam_km / 2.0;
        let depth = |lo: f64| ring_mean(&hm, lo, lat, r * 0.95, r * 1.1) - ring_mean(&hm, lo, lat, r * 0.35, r * 0.6);
        let (here, mirror) = (depth(lon), depth(-lon));
        let ok = here > 1500.0 && here > 2.0 * mirror.abs();
        checks.push(format!(
            "{}: Krater {name} — Rand liegt {:.0} m über dem Boden (am Spiegelort: {:.0} m)",
            if ok { "besteht" } else { "VERFEHLT" },
            here,
            mirror
        ));
    }

    println!("berechne Neigungen …");
    let relief = build_levels(slopes(&hm), w, h, halve_rg16f);
    lmap::write(&paths.forged().join("relief.lmap"), w, h, LmapFormat::Rg16Float, &relief)?;
    drop(relief);

    println!("berechne Horizontkarten ({} Richtungen, {} Threads) …", HORIZON_DIRS, rayon::current_num_threads());
    let th = Instant::now();
    let (ha, hb, reach_km) = horizons(&hm);
    println!("  {:.1} s, Suchweite {:.0} km", th.elapsed().as_secs_f64(), reach_km);
    let la = build_levels(ha, w, h, |s, w, h| halve_rgba8(s, w, h, false));
    lmap::write(&paths.forged().join("horizon_a.lmap"), w, h, LmapFormat::Rgba8, &la)?;
    drop(la);
    let lb = build_levels(hb, w, h, |s, w, h| halve_rgba8(s, w, h, false));
    lmap::write(&paths.forged().join("horizon_b.lmap"), w, h, LmapFormat::Rgba8, &lb)?;
    drop(lb);

    // --- Farbe ---
    println!("lese Farbkarte …");
    let (cw, chh, cch, craw) = read_tiff_u16(&paths.raw().join("lroc_color_16bit_srgb_8k.tif"))?;
    if cch < 3 || cw != 2 * chh {
        bail!("Farbkarte: unerwartetes Format {cw}×{chh}, {cch} Kanäle");
    }
    let mut rgba = vec![255u8; cw as usize * chh as usize * 4];
    rgba.par_chunks_mut(4).zip(craw.par_chunks(cch)).for_each(|(o, i)| {
        for c in 0..3 {
            o[c] = ((i[c] as u32 * 255 + 32767) / 65535) as u8;
        }
    });
    drop(craw);
    // Flächengewichteter Mittelwert der linearen Leuchtdichte.
    let (cwu, chu) = (cw as usize, chh as usize);
    let (sum, wsum) = rgba
        .par_chunks(cwu * 4)
        .enumerate()
        .map(|(y, row)| {
            let wgt = ((90.0 - (y as f64 + 0.5) * 180.0 / chu as f64) * PI / 180.0).cos();
            let mut s = 0.0;
            for px in row.chunks(4) {
                s += 0.2126 * srgb_to_linear(px[0] as f64 / 255.0)
                    + 0.7152 * srgb_to_linear(px[1] as f64 / 255.0)
                    + 0.0722 * srgb_to_linear(px[2] as f64 / 255.0);
            }
            (s * wgt, wgt * cwu as f64)
        })
        .reduce(|| (0.0, 0.0), |a, b| (a.0 + b.0, a.1 + b.1));
    let albedo_mean_linear = sum / wsum;
    println!("  {}×{}, mittlere lineare Leuchtdichte {:.4}", cw, chh, albedo_mean_linear);
    // Kartenlage der Farbkarte: dunkle Kraterböden an ihrem Ort, nicht am Ost-West-Spiegelbild.
    // Verglichen wird jeweils der dunkelste Fleck (0,3° Halbmesser) im Umkreis von 1,5°,
    // damit ein heller Zentralberg genau in der Kratermitte das Ergebnis nicht verfälscht.
    let darkest = |lon: f64, lat: f64| -> f64 {
        let mut best = f64::INFINITY;
        for iy in -5..=5 {
            for ix in -5..=5 {
                let (dx, dy) = (ix as f64 * 0.3, iy as f64 * 0.3);
                if dx * dx + dy * dy <= 1.5 * 1.5 {
                    best = best.min(albedo_lum(&rgba, cwu, chu, lon + dx / (lat * PI / 180.0).cos(), lat + dy, 0.3));
                }
            }
        }
        best
    };
    for (name, lat, lon) in [("Plato", 51.6, -9.4), ("Grimaldi", -5.5, -68.3), ("Tsiolkovskiy", -20.4, 129.1)] {
        let here = darkest(lon, lat);
        let mirror = darkest(-lon, lat);
        let ok = here < 0.75 * mirror;
        checks.push(format!(
            "{}: Farbkarte, dunkler Boden von {name} — Leuchtdichte {:.3} (am Spiegelort: {:.3})",
            if ok { "besteht" } else { "VERFEHLT" },
            here,
            mirror
        ));
    }
    let albedo = build_levels(rgba, cw, chh, |s, w, h| halve_rgba8(s, w, h, true));
    lmap::write(&paths.forged().join("albedo.lmap"), cw, chh, LmapFormat::Rgba8Srgb, &albedo)?;
    drop(albedo);

    // --- Manifest ---
    println!("schreibe Manifest …");
    let raw = raw_entries(&paths)?;
    let stamp = now_utc();
    let forged_entry = |name: &str, from: &str, steps: &[&str]| -> Result<ManifestEntry> {
        let rel = format!("forged/{name}");
        let (sha256, bytes) = sha256_file(&paths.root.join(&rel))?;
        Ok(ManifestEntry {
            path: rel,
            sha256,
            bytes,
            source: "luna-forge".to_string(),
            product: format!("abgeleitet aus {from}"),
            retrieved_utc: stamp.clone(),
            steps: steps.iter().map(|s| s.to_string()).collect(),
        })
    };
    let forged = vec![
        forged_entry("albedo.lmap", "raw/lroc_color_16bit_srgb_8k.tif", &[
            "16 Bit sRGB auf 8 Bit sRGB gerundet",
            "Verkleinerungskette durch 2×2-Mittelung im linearen Licht",
        ])?,
        forged_entry("relief.lmap", "raw/ldem_16_uint.tif", &[
            "Höhe = Wert × 0,5 m − 10 000 m über der Kugel mit 1737,4 km",
            "Neigungen nach Ost und Nord aus zentralen Differenzen; Ost-West-Stützweite wächst zu den Polen mit 1/cos(Breite)",
            "als 16-Bit-Gleitkomma gespeichert, Verkleinerungskette durch 2×2-Mittelung",
        ])?,
        forged_entry("horizon_a.lmap", "raw/ldem_16_uint.tif", &[
            "Horizontwinkel in den Richtungen N, NO, O, SO entlang von Großkreisen, Kugelkrümmung berücksichtigt",
            "Suche: 24 Schritte je ein Äquatortexel, danach Faktor 1,12 je Schritt bis rund 250 km; ferne Schritte auf 4×4-gemittelten Höhen",
            "Winkel als sign(a)·sqrt(|a|/45°) in 8 Bit kodiert",
        ])?,
        forged_entry("horizon_b.lmap", "raw/ldem_16_uint.tif", &["wie horizon_a.lmap, Richtungen S, SW, W, NW"])?,
    ];
    let failed = checks.iter().filter(|c| c.starts_with("VERFEHLT")).count();
    let manifest = Manifest {
        schema: 1,
        raw,
        forged,
        forge: ForgeInfo {
            forge_version: env!("CARGO_PKG_VERSION").to_string(),
            albedo_mean_linear,
            horizon_max_angle_deg: HORIZON_MAX_ANGLE_DEG,
            horizon_directions: HORIZON_DIRS as u32,
            horizon_reach_km: reach_km,
            height_min_m: hmin,
            height_max_m: hmax,
            height_texel_km: MOON_RADIUS_KM * 2.0 * PI / w as f64,
            albedo_texel_km: MOON_RADIUS_KM * 2.0 * PI / cw as f64,
            checks: checks.clone(),
        },
    };
    manifest.save(&paths)?;

    println!("\nKartenlage:");
    for c in &checks {
        println!("  {c}");
    }
    println!("\nfertig in {:.1} s → {}", t0.elapsed().as_secs_f64(), paths.forged().display());
    if failed > 0 {
        bail!("{failed} Prüfung(en) der Kartenlage verfehlt");
    }
    Ok(())
}
