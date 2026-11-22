//! Der Szenenzustand: alles, was LUNA zu einem Zeitpunkt und Standort weiß.
//!
//! Renderer und Oberfläche lesen ihn nur. Jede angezeigte Größe trägt ihr
//! Herkunftskennzeichen (M, B, N, K) samt Quelle und Fehlergrenze.

use anyhow::Result;
use hifitime::{Epoch, Unit};

use crate::eph::*;
use crate::math::*;
use crate::time;

/// Herkunft einer Größe.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// Gemessen: Instrumentendaten.
    M,
    /// Berechnet: aus Ephemeriden und Geometrie.
    B,
    /// Näherung: vereinfachtes Modell mit bekannter Fehlergrenze.
    N,
    /// Künstlerisch: Gestaltung ohne Messanspruch.
    K,
}

impl Kind {
    pub fn letter(self) -> &'static str {
        match self {
            Kind::M => "M",
            Kind::B => "B",
            Kind::N => "N",
            Kind::K => "K",
        }
    }
    pub fn word(self) -> &'static str {
        match self {
            Kind::M => "gemessen",
            Kind::B => "berechnet",
            Kind::N => "Näherung",
            Kind::K => "künstlerisch",
        }
    }
}

/// Eine angezeigte Größe mit Herkunft.
#[derive(Clone, Debug)]
pub struct Reading {
    pub label: &'static str,
    pub value: f64,
    pub unit: &'static str,
    pub decimals: usize,
    pub signed: bool,
    pub kind: Kind,
    pub source: &'static str,
    pub bound: &'static str,
}

impl Reading {
    pub fn value_text(&self) -> String {
        if self.signed {
            format!("{:+.*}", self.decimals, self.value)
        } else {
            format!("{:.*}", self.decimals, self.value)
        }
    }
}

#[derive(Clone, Debug)]
pub struct Scene {
    pub epoch: Epoch,
    /// Bürgerliches Julianisches Datum (ab 1972 UTC, davor UT1).
    pub civil_jd: f64,
    pub site: Site,
    pub earth_rotation: EarthRotation,

    // --- Vektoren, Achsen ICRF/J2000 ---
    /// Mondmitte relativ zum Beobachter, lichtlaufzeitkorrigiert (km).
    pub moon_from_obs_km: V3,
    /// Scheinbare Richtung zum Mond (mit Aberration), Einheitsvektor.
    pub moon_dir_apparent: V3,
    /// Sonne relativ zur Mondmitte, zu dem Zeitpunkt, als das Licht den Mond verließ (km).
    pub sun_from_moon_km: V3,
    /// Scheinbare Richtung zur Sonne vom Beobachter, Einheitsvektor.
    pub sun_dir_apparent: V3,
    /// Drehmatrix Kartensystem des Mondes (ME) → J2000, zum Zeitpunkt der Lichtaussendung.
    pub moon_me_to_j2000: M3,
    /// Lokale Achsen des Beobachters in J2000.
    pub east: V3,
    pub north: V3,
    pub zenith: V3,
    /// Wahrer Himmelsnordpol des Datums in J2000.
    pub celestial_pole: V3,

    // --- Abgeleitete Größen ---
    pub light_time_s: f64,
    pub distance_km: f64,
    pub geocentric_distance_km: f64,
    pub sun_distance_km: f64,
    pub ra_icrf_deg: f64,
    pub dec_icrf_deg: f64,
    pub ra_apparent_deg: f64,
    pub dec_apparent_deg: f64,
    pub azimuth_deg: f64,
    /// Höhe über dem Horizont ohne Atmosphäre.
    pub altitude_deg: f64,
    /// Höhe mit Standardrefraktion (Näherung).
    pub altitude_refracted_deg: f64,
    pub angular_diameter_arcsec: f64,
    pub phase_angle_deg: f64,
    /// Beleuchteter Anteil der Scheibe, 0…1.
    pub illuminated_fraction: f64,
    pub elongation_deg: f64,
    pub waxing: bool,
    /// Libration: selenografische Länge/Breite des Punktes unter dem Beobachter.
    pub libration_lon_deg: f64,
    pub libration_lat_deg: f64,
    pub subsolar_lon_deg: f64,
    pub subsolar_lat_deg: f64,
    /// Positionswinkel der Mondachse (Nordpol), vom Himmelsnordpol des Datums über Ost.
    pub axis_position_angle_deg: f64,
    /// Positionswinkel der hellen Kante (Richtung zur Sonne).
    pub bright_limb_angle_deg: f64,
    /// Parallaktischer Winkel: Positionswinkel des Zenits.
    pub parallactic_angle_deg: f64,
    pub sun_altitude_deg: f64,
}

/// Standardrefraktion nach Sæmundsson für 1010 hPa und 10 °C, in Grad.
/// Eingabe ist die Höhe ohne Atmosphäre. Näherung; nahe dem Horizont wetterabhängig.
pub fn refraction_deg(true_alt_deg: f64) -> f64 {
    if true_alt_deg < -1.0 {
        return 0.0;
    }
    let h = true_alt_deg;
    let r_arcmin = 1.02 / ((h + 10.3 / (h + 5.11)) * DEG).tan();
    (r_arcmin / 60.0).max(0.0)
}

/// Positionswinkel der Richtung `v` an der Himmelsstelle `dir`, gemessen von `pole` über Ost.
fn position_angle_deg(dir: V3, pole: V3, v: V3) -> f64 {
    let east = unit(cross(pole, dir));
    let north = cross(dir, east);
    wrap360(dot(v, east).atan2(dot(v, north)) / DEG)
}

/// Relativistische Aberration: wahre Richtung `p` → scheinbare Richtung für einen
/// Beobachter mit Geschwindigkeit `v` (km/s) relativ zum Schwerpunkt des Sonnensystems.
fn aberrate(p: V3, v_km_s: V3) -> V3 {
    let beta = scale(v_km_s, 1.0 / C_KM_S);
    let b2 = dot(beta, beta);
    let inv_gamma = (1.0 - b2).sqrt();
    let pb = dot(p, beta);
    let f = 1.0 + pb / (1.0 + inv_gamma);
    unit(add(scale(p, inv_gamma), scale(beta, f)))
}

impl Scene {
    pub fn compute(eph: &Ephemeris, site: Site, epoch: Epoch) -> Result<Self> {
        eph.check_epoch(epoch)?;
        let sec = |s: f64| Unit::Second * s;

        // 1. Beobachter: erdfest → J2000, dazu Geschwindigkeit für die Aberration.
        let (earth_rot, earth_rotation) = eph.earth_itrf_to_j2000(epoch)?;
        let (earth_pos, earth_vel) = eph.state_ssb(Body::Earth, epoch)?;
        let site_j2000 = mul_mv(&earth_rot, site.itrf_km());
        let obs_pos = add(earth_pos, site_j2000);
        let obs_vel = add(earth_vel, rotation_velocity(&earth_rot, site_j2000));
        let (e_itrf, n_itrf, z_itrf) = site.enu_itrf();
        let east = mul_mv(&earth_rot, e_itrf);
        let north = mul_mv(&earth_rot, n_itrf);
        let zenith = mul_mv(&earth_rot, z_itrf);
        let celestial_pole = {
            let tod = true_of_date_from_j2000(epoch);
            tod[2] // dritte Zeile: z-Achse des Datums, ausgedrückt in J2000
        };

        // 2. Mond mit Lichtlaufzeit.
        let mut lt = 1.28;
        let mut moon_pos = [0.0; 3];
        let mut moon_vel = [0.0; 3];
        for _ in 0..4 {
            (moon_pos, moon_vel) = eph.state_ssb(Body::Moon, epoch - sec(lt))?;
            lt = norm(sub(moon_pos, obs_pos)) / C_KM_S;
        }
        let t_moon = epoch - sec(lt);
        let moon_from_obs = sub(moon_pos, obs_pos);
        let distance_km = norm(moon_from_obs);
        let moon_dir_true = unit(moon_from_obs);
        let moon_dir_apparent = aberrate(moon_dir_true, obs_vel);
        let geocentric_distance_km = norm(sub(moon_pos, earth_pos));

        // 3. Sonne: Licht, das den Mond zum Zeitpunkt t_moon trifft.
        let mut lt_sun = 499.0;
        let mut sun_from_moon = [0.0; 3];
        for _ in 0..4 {
            let sun_pos = eph.state_ssb(Body::Sun, t_moon - sec(lt_sun))?.0;
            sun_from_moon = sub(sun_pos, moon_pos);
            lt_sun = norm(sun_from_moon) / C_KM_S;
        }
        // Der Mond bewegt sich mit rund 30 km/s um die Sonne. Welche Punkte seiner
        // Oberfläche das Licht trifft, entscheidet die Lichtrichtung im Ruhesystem des
        // Mondes: Die Sonnenrichtung ist um bis zu 20,5″ aberriert (etwa 170 m an der Lichtgrenze).
        let sun_distance_km = norm(sun_from_moon);
        let sun_from_moon = scale(aberrate(unit(sun_from_moon), moon_vel), sun_distance_km);
        // Sonne vom Beobachter aus (für Elongation und Sonnenhöhe).
        let mut lt_so = 499.0;
        let mut sun_from_obs = [0.0; 3];
        for _ in 0..4 {
            let sun_pos = eph.state_ssb(Body::Sun, epoch - sec(lt_so))?.0;
            sun_from_obs = sub(sun_pos, obs_pos);
            lt_so = norm(sun_from_obs) / C_KM_S;
        }
        let sun_dir_apparent = aberrate(unit(sun_from_obs), obs_vel);

        // 4. Drehlage des Mondes zum Zeitpunkt der Lichtaussendung.
        let moon_me_to_j2000 = eph.moon_me_to_j2000(t_moon)?;
        let j2000_to_me = transpose(&moon_me_to_j2000);

        // 5. Abgeleitete Größen.
        let (ra, dec) = lon_lat_deg(moon_from_obs);
        let tod = true_of_date_from_j2000(epoch);
        let (ra_app, dec_app) = lon_lat_deg(mul_mv(&tod, moon_dir_apparent));
        let altitude_deg = dot(moon_dir_apparent, zenith).clamp(-1.0, 1.0).asin() / DEG;
        let azimuth_deg =
            wrap360(dot(moon_dir_apparent, east).atan2(dot(moon_dir_apparent, north)) / DEG);
        let sun_altitude_deg = dot(sun_dir_apparent, zenith).clamp(-1.0, 1.0).asin() / DEG;

        let to_obs = scale(moon_from_obs, -1.0);
        let phase_angle = angle(sun_from_moon, to_obs);
        let illuminated_fraction = 0.5 * (1.0 + phase_angle.cos());
        let elongation_deg = angle(sun_dir_apparent, moon_dir_apparent) / DEG;
        // Zunehmend, solange der Mond östlich der Sonne steht (bezogen auf die Ekliptik).
        let ecl_north = {
            let eps = 84381.406 * ARCSEC;
            [0.0, -eps.sin(), eps.cos()]
        };
        let waxing = dot(cross(sun_from_obs, moon_from_obs), ecl_north) > 0.0;

        let (libration_lon_deg, libration_lat_deg) = lon_lat_deg(mul_mv(&j2000_to_me, to_obs));
        let (subsolar_lon_deg, subsolar_lat_deg) = lon_lat_deg(mul_mv(&j2000_to_me, sun_from_moon));

        let moon_pole = mul_mv(&moon_me_to_j2000, [0.0, 0.0, 1.0]);
        let axis_position_angle_deg = position_angle_deg(moon_dir_true, celestial_pole, moon_pole);
        let bright_limb_angle_deg = position_angle_deg(moon_dir_true, celestial_pole, unit(sun_from_moon));
        let parallactic_angle_deg = wrap180(position_angle_deg(moon_dir_apparent, celestial_pole, zenith));
        let angular_diameter_arcsec = 2.0 * (MOON_RADIUS_KM / distance_km).asin() / ARCSEC;

        Ok(Scene {
            epoch,
            civil_jd: time::civil_jd(epoch),
            site,
            earth_rotation,
            moon_from_obs_km: moon_from_obs,
            moon_dir_apparent,
            sun_from_moon_km: sun_from_moon,
            sun_dir_apparent,
            moon_me_to_j2000,
            east,
            north,
            zenith,
            celestial_pole,
            light_time_s: lt,
            distance_km,
            geocentric_distance_km,
            sun_distance_km,
            ra_icrf_deg: wrap360(ra),
            dec_icrf_deg: dec,
            ra_apparent_deg: wrap360(ra_app),
            dec_apparent_deg: dec_app,
            azimuth_deg,
            altitude_deg,
            altitude_refracted_deg: altitude_deg + refraction_deg(altitude_deg),
            angular_diameter_arcsec,
            phase_angle_deg: phase_angle / DEG,
            illuminated_fraction,
            elongation_deg,
            waxing,
            libration_lon_deg,
            libration_lat_deg,
            subsolar_lon_deg,
            subsolar_lat_deg,
            axis_position_angle_deg,
            bright_limb_angle_deg,
            parallactic_angle_deg,
            sun_altitude_deg,
        })
    }

    /// Neigung der Mondachse gegen die Senkrechte, wie man sie am Himmel sieht:
    /// Winkel vom Zenit zum Mondnordpol, positiv gegen den Uhrzeigersinn.
    pub fn axis_tilt_from_zenith_deg(&self) -> f64 {
        wrap180(self.axis_position_angle_deg - self.parallactic_angle_deg)
    }

    /// Selenografische Colongitude der Sonne: Länge der Morgen-Lichtgrenze, 0…360° nach Westen.
    pub fn colongitude_deg(&self) -> f64 {
        wrap360(90.0 - self.subsolar_lon_deg)
    }

    pub fn phase_name(&self) -> &'static str {
        let k = self.illuminated_fraction;
        match (k, self.waxing) {
            (k, _) if k < 0.005 => "Neumond",
            (k, _) if k > 0.995 => "Vollmond",
            (k, true) if (k - 0.5).abs() < 0.03 => "Erstes Viertel",
            (k, false) if (k - 0.5).abs() < 0.03 => "Letztes Viertel",
            (k, true) if k < 0.5 => "Zunehmende Sichel",
            (_, true) => "Zunehmender Mond",
            (k, false) if k < 0.5 => "Abnehmende Sichel",
            (_, false) => "Abnehmender Mond",
        }
    }

    /// Die acht Messwerte des Prototyps, jeder mit Herkunftskennzeichen.
    pub fn readings(&self) -> Vec<Reading> {
        const EPH: &str = "JPL DE440 (de440s.bsp), Lichtlaufzeit berücksichtigt";
        const ROT: &str = "JPL DE440 Monddrehlage (moon_pa_de440), ins Kartensystem ME gedreht";
        let earth_bound = match self.earth_rotation {
            EarthRotation::Kernel => "geprüft gegen JPL Horizons: ≤ 1″",
            EarthRotation::ModelUtc => "Erddrehung außerhalb der Messreihe: bis etwa 15″",
            EarthRotation::ModelDeltaT => "Erddrehung aus ΔT-Modell: bis etwa 1′",
        };
        vec![
            Reading {
                label: "BELEUCHTUNG",
                value: self.illuminated_fraction * 100.0,
                unit: "%",
                decimals: 2,
                signed: false,
                kind: Kind::B,
                source: EPH,
                bound: "geprüft gegen JPL Horizons: ≤ 0,01 Prozentpunkte",
            },
            Reading {
                label: "PHASENWINKEL",
                value: self.phase_angle_deg,
                unit: "°",
                decimals: 3,
                signed: false,
                kind: Kind::B,
                source: EPH,
                bound: "geprüft gegen JPL Horizons: ≤ 0,01°",
            },
            Reading {
                label: "ENTFERNUNG",
                value: self.distance_km,
                unit: "km",
                decimals: 0,
                signed: false,
                kind: Kind::B,
                source: "JPL DE440, Beobachter bis Mondmitte",
                bound: "geprüft gegen JPL Horizons: ≤ 1 km",
            },
            Reading {
                label: "DURCHMESSER",
                value: self.angular_diameter_arcsec / 60.0,
                unit: "'",
                decimals: 3,
                signed: false,
                kind: Kind::B,
                source: "aus Entfernung und Mondradius 1737,4 km",
                bound: "geprüft gegen JPL Horizons: ≤ 0,01″",
            },
            Reading {
                label: "LIBRATION L",
                value: self.libration_lon_deg,
                unit: "°",
                decimals: 3,
                signed: true,
                kind: Kind::B,
                source: ROT,
                bound: "geprüft gegen JPL Horizons: ≤ 0,01°",
            },
            Reading {
                label: "LIBRATION B",
                value: self.libration_lat_deg,
                unit: "°",
                decimals: 3,
                signed: true,
                kind: Kind::B,
                source: ROT,
                bound: "geprüft gegen JPL Horizons: ≤ 0,01°",
            },
            Reading {
                label: "ACHSLAGE",
                value: self.axis_position_angle_deg,
                unit: "°",
                decimals: 2,
                signed: false,
                kind: Kind::B,
                source: "Positionswinkel des Mondnordpols, vom Himmelsnordpol über Ost",
                bound: "geprüft gegen JPL Horizons: ≤ 0,01°",
            },
            Reading {
                label: "HÖHE",
                value: self.altitude_deg,
                unit: "°",
                decimals: 3,
                signed: true,
                kind: Kind::B,
                source: "Höhe über dem Horizont ohne Atmosphäre",
                bound: earth_bound,
            },
        ]
    }
}
