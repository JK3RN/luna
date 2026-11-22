//! Schmale Schnittstelle zu den JPL-Kernen (über ANISE) und zur Erddrehung.
//!
//! Alles, was LUNA von Ephemeriden braucht, läuft durch diese Datei:
//! baryzentrische Zustände von Sonne, Erde und Mond, die Drehlage des Mondes
//! (Hauptachsensystem aus DE440, fest ins Kartensystem "Mittlere Erde / Polachse"
//! gedreht) und die Drehlage der Erde.

use std::path::Path;

use anise::constants::frames::{EARTH_J2000, MOON_J2000, SSB_J2000, SUN_J2000};
use anise::constants::orientations::{ITRF93, J2000, MOON_PA_DE440};
use anise::prelude::{Almanac, Frame};
use anyhow::{anyhow, bail, Context, Result};
use hifitime::Epoch;

use crate::math::*;
use crate::time;

pub const C_KM_S: f64 = 299_792.458;
/// Bezugsradius aller LRO-Daten und der IAU-Wert (pck00011.tpc, BODY301_RADII).
pub const MOON_RADIUS_KM: f64 = 1737.4;
/// BODY10_RADII aus pck00011.tpc.
pub const SUN_RADIUS_KM: f64 = 695_700.0;
pub const AU_KM: f64 = 149_597_870.7;
/// WGS84, wie von JPL Horizons für Standorte verwendet.
pub const EARTH_A_KM: f64 = 6378.137;
pub const EARTH_F: f64 = 1.0 / 298.257_223_563;
const EARTH_OMEGA_RAD_S: f64 = 7.292_115_146_7e-5;

/// Feste Drehung MOON_PA_DE440 → MOON_ME_DE440_ME421 laut `moon_de440_250416.tf`:
/// TKFRAME_31009_ANGLES = (67.8526, 78.6944, 0.2785) Bogensekunden, AXES = (3, 2, 1),
/// definiert als Drehung vom ME-System ins PA-System.
const ME_TO_PA_ARCSEC: [f64; 3] = [67.8526, 78.6944, 0.2785];

const NAIF_EARTH: i32 = 399;
const NAIF_MOON: i32 = 301;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Body {
    Sun,
    Earth,
    Moon,
}

/// Woher die Drehlage der Erde für einen Zeitpunkt stammt.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EarthRotation {
    /// Gemessene bzw. von JPL vorhergesagte Erddrehung aus `earth_latest_high_prec.bpc`.
    Kernel,
    /// IAU 2006/2000A mit UT1 = UTC (ab 1972 außerhalb der Messreihe). Fehler ≤ 0,9 s.
    ModelUtc,
    /// IAU 2006/2000A mit UT1 aus dem ΔT-Modell (vor 1972).
    ModelDeltaT,
}

impl EarthRotation {
    pub fn label(self) -> &'static str {
        match self {
            EarthRotation::Kernel => "Messreihe (JPL-Kern)",
            EarthRotation::ModelUtc => "Modell IAU 2006, UT1≈UTC",
            EarthRotation::ModelDeltaT => "Modell IAU 2006, ΔT-Näherung",
        }
    }
}

pub struct Ephemeris {
    almanac: Almanac,
    spk_domain: (Epoch, Epoch),
    moon_rot_domain: (Epoch, Epoch),
    earth_rot_domain: (Epoch, Epoch),
    me_to_pa: M3,
}

impl Ephemeris {
    /// Lädt `de440s.bsp`, `moon_pa_de440_200625.bpc` und `earth_latest_high_prec.bpc` aus `dir`.
    pub fn load(dir: &Path) -> Result<Self> {
        let mut almanac = Almanac::default();
        for name in ["de440s.bsp", "moon_pa_de440_200625.bpc", "earth_latest_high_prec.bpc"] {
            let p = dir.join(name);
            if !p.is_file() {
                bail!(
                    "Kern fehlt: {} — bitte einmal scripts/fetch-data.sh ausführen",
                    p.display()
                );
            }
            let s = p.to_str().ok_or_else(|| anyhow!("Pfad nicht als Text darstellbar"))?;
            almanac = almanac.load(s).map_err(|e| anyhow!("{name}: {e}"))?;
        }
        let dom = |r: (Epoch, Epoch), s: (Epoch, Epoch)| (r.0.max(s.0), r.1.min(s.1));
        let moon = almanac.spk_domain(NAIF_MOON).map_err(|e| anyhow!("{e}"))?;
        let earth = almanac.spk_domain(NAIF_EARTH).map_err(|e| anyhow!("{e}"))?;
        let sun = almanac.spk_domain(10).map_err(|e| anyhow!("{e}"))?;
        let spk_domain = dom(dom(moon, earth), sun);
        let moon_rot_domain = almanac.bpc_domain(MOON_PA_DE440).map_err(|e| anyhow!("{e}"))?;
        let earth_rot_domain = almanac.bpc_domain(ITRF93).map_err(|e| anyhow!("{e}"))?;

        let a = ME_TO_PA_ARCSEC;
        let me_to_pa = mul_mm(
            &rot_axis(3, a[0] * ARCSEC),
            &mul_mm(&rot_axis(2, a[1] * ARCSEC), &rot_axis(1, a[2] * ARCSEC)),
        );
        Ok(Self { almanac, spk_domain, moon_rot_domain, earth_rot_domain, me_to_pa })
    }

    /// Zeitraum, in dem Positionen und Monddrehlage gemeinsam vorliegen
    /// (mit einem Tag Abstand zu den Rändern für die Lichtlaufzeit).
    pub fn domain(&self) -> (Epoch, Epoch) {
        let day = hifitime::Unit::Day * 1;
        (
            self.spk_domain.0.max(self.moon_rot_domain.0) + day,
            self.spk_domain.1.min(self.moon_rot_domain.1) - day,
        )
    }

    /// Zeitraum der Erddrehungs-Messreihe.
    pub fn earth_rot_domain(&self) -> (Epoch, Epoch) {
        self.earth_rot_domain
    }

    pub fn check_epoch(&self, epoch: Epoch) -> Result<()> {
        let (a, b) = self.domain();
        if epoch < a || epoch > b {
            bail!(
                "Zeitpunkt {} liegt außerhalb der Ephemeride DE440s ({} bis {})",
                time::format_civil(time::civil_jd(epoch)),
                time::format_civil(time::civil_jd(a)),
                time::format_civil(time::civil_jd(b))
            );
        }
        Ok(())
    }

    /// Geometrischer Zustand (Position km, Geschwindigkeit km/s) relativ zum
    /// Schwerpunkt des Sonnensystems, Achsen ICRF/J2000.
    pub fn state_ssb(&self, body: Body, epoch: Epoch) -> Result<(V3, V3)> {
        let frame = match body {
            Body::Sun => SUN_J2000,
            Body::Earth => EARTH_J2000,
            Body::Moon => MOON_J2000,
        };
        let s = self
            .almanac
            .translate(frame, SSB_J2000, epoch, None)
            .map_err(|e| anyhow!("Ephemeride {body:?}: {e}"))?;
        Ok((
            [s.radius_km.x, s.radius_km.y, s.radius_km.z],
            [s.velocity_km_s.x, s.velocity_km_s.y, s.velocity_km_s.z],
        ))
    }

    fn rotation(&self, from: Frame, to: Frame, epoch: Epoch) -> Result<M3> {
        let dcm = self.almanac.rotate(from, to, epoch).map_err(|e| anyhow!("Drehlage: {e}"))?;
        let m = dcm.rot_mat;
        Ok([
            [m[(0, 0)], m[(0, 1)], m[(0, 2)]],
            [m[(1, 0)], m[(1, 1)], m[(1, 2)]],
            [m[(2, 0)], m[(2, 1)], m[(2, 2)]],
        ])
    }

    /// Drehmatrix Kartensystem des Mondes (ME) → ICRF/J2000.
    pub fn moon_me_to_j2000(&self, epoch: Epoch) -> Result<M3> {
        let pa_to_j = self
            .rotation(Frame::new(NAIF_MOON, MOON_PA_DE440), Frame::new(NAIF_MOON, J2000), epoch)
            .context("Monddrehlage (moon_pa_de440)")?;
        Ok(mul_mm(&pa_to_j, &self.me_to_pa))
    }

    /// Drehmatrix Hauptachsensystem (PA) → Kartensystem (ME); nur für Prüfungen.
    pub fn pa_to_me(&self) -> M3 {
        transpose(&self.me_to_pa)
    }

    /// Drehmatrix erdfest (ITRF93) → ICRF/J2000 und ihre Herkunft.
    pub fn earth_itrf_to_j2000(&self, epoch: Epoch) -> Result<(M3, EarthRotation)> {
        let margin = hifitime::Unit::Minute * 5;
        if epoch > self.earth_rot_domain.0 + margin && epoch < self.earth_rot_domain.1 - margin {
            let m = self
                .rotation(Frame::new(NAIF_EARTH, ITRF93), Frame::new(NAIF_EARTH, J2000), epoch)
                .context("Erddrehung (earth_latest_high_prec)")?;
            return Ok((m, EarthRotation::Kernel));
        }
        let src = if time::civil_jd(epoch) >= time::JD_UTC_START {
            EarthRotation::ModelUtc
        } else {
            EarthRotation::ModelDeltaT
        };
        Ok((earth_model_itrf_to_j2000(epoch), src))
    }
}

/// Ausweichmodell: IAU 2006/2000A ohne Polbewegung, UT1 aus `time::ut1_jd_model`.
pub fn earth_model_itrf_to_j2000(epoch: Epoch) -> M3 {
    let tt = epoch.to_jde_tt_days();
    let ut1 = time::ut1_jd_model(epoch);
    let (tta, ttb) = (tt.floor(), tt - tt.floor());
    let (uta, utb) = (ut1.floor(), ut1 - ut1.floor());
    let c2t = sofars::pnp::c2t06a(tta, ttb, uta, utb, 0.0, 0.0);
    transpose(&c2t)
}

/// Präzession-Nutation IAU 2006/2000A: ICRF → wahrer Äquator und Frühlingspunkt des Datums.
pub fn true_of_date_from_j2000(epoch: Epoch) -> M3 {
    let tt = epoch.to_jde_tt_days();
    sofars::pnp::pnm06a(tt.floor(), tt - tt.floor())
}

/// Beobachtungsort auf der Erde.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Site {
    /// Geodätische Breite in Grad, Nord positiv.
    pub lat_deg: f64,
    /// Länge in Grad, Ost positiv.
    pub lon_deg: f64,
    /// Höhe über dem Ellipsoid in Metern.
    pub height_m: f64,
}

impl Site {
    pub fn new(lat_deg: f64, lon_deg: f64, height_m: f64) -> Result<Self> {
        if !(-90.0..=90.0).contains(&lat_deg) {
            bail!("Breite {lat_deg}° liegt außerhalb von −90° bis +90°");
        }
        if !(-360.0..=360.0).contains(&lon_deg) {
            bail!("Länge {lon_deg}° liegt außerhalb von −360° bis +360°");
        }
        if !(-500.0..=10_000.0).contains(&height_m) {
            bail!("Höhe {height_m} m liegt außerhalb von −500 m bis 10 000 m");
        }
        Ok(Self { lat_deg, lon_deg, height_m })
    }

    /// Erdfeste Position in km (WGS84).
    pub fn itrf_km(&self) -> V3 {
        let (sp, cp) = (self.lat_deg * DEG).sin_cos();
        let (sl, cl) = (self.lon_deg * DEG).sin_cos();
        let e2 = EARTH_F * (2.0 - EARTH_F);
        let n = EARTH_A_KM / (1.0 - e2 * sp * sp).sqrt();
        let h = self.height_m / 1000.0;
        [(n + h) * cp * cl, (n + h) * cp * sl, (n * (1.0 - e2) + h) * sp]
    }

    /// Lokale Achsen (Ost, Nord, Zenit) im erdfesten System.
    pub fn enu_itrf(&self) -> (V3, V3, V3) {
        let (sp, cp) = (self.lat_deg * DEG).sin_cos();
        let (sl, cl) = (self.lon_deg * DEG).sin_cos();
        ([-sl, cl, 0.0], [-sp * cl, -sp * sl, cp], [cp * cl, cp * sl, sp])
    }
}

/// Geschwindigkeit eines erdfesten Punktes durch die Erddrehung, in J2000 (km/s).
pub(crate) fn rotation_velocity(itrf_to_j2000: &M3, r_j2000: V3) -> V3 {
    let axis = mul_mv(itrf_to_j2000, [0.0, 0.0, 1.0]);
    cross(scale(axis, EARTH_OMEGA_RAD_S), r_j2000)
}
