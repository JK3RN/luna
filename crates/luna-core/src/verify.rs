//! Selbstprüfung: vergleicht die Rechenkette mit eingefrorenen Referenzwerten
//! von JPL Horizons (reference/horizons/*.txt). Läuft ohne Netz.

use std::collections::HashMap;
use std::path::Path;

use anyhow::{anyhow, bail, Context, Result};

use crate::eph::{EarthRotation, Ephemeris, Site};
use crate::math::*;
use crate::scene::Scene;
use crate::time;

/// Eine Zeile aus einer Horizons-Tabelle.
#[derive(Clone, Debug)]
pub struct RefRow {
    pub jd_ut: f64,
    pub values: HashMap<String, f64>,
}

#[derive(Clone, Debug)]
pub struct RefFile {
    pub name: String,
    pub site: Site,
    pub rows: Vec<RefRow>,
}

pub fn parse_horizons(name: &str, text: &str) -> Result<RefFile> {
    let geo = text
        .lines()
        .find(|l| l.starts_with("Center geodetic"))
        .ok_or_else(|| anyhow!("{name}: Zeile 'Center geodetic' fehlt"))?;
    let nums: Vec<f64> = geo
        .split(':')
        .nth(1)
        .unwrap_or("")
        .split('{')
        .next()
        .unwrap_or("")
        .split(',')
        .filter_map(|s| s.trim().parse::<f64>().ok())
        .collect();
    if nums.len() != 3 {
        bail!("{name}: Standort nicht lesbar: '{geo}'");
    }
    let site = Site::new(nums[1], nums[0], nums[2] * 1000.0)?;

    let lines: Vec<&str> = text.lines().collect();
    let soe = lines.iter().position(|l| l.starts_with("$$SOE")).ok_or_else(|| anyhow!("{name}: $$SOE fehlt"))?;
    let eoe = lines.iter().position(|l| l.starts_with("$$EOE")).ok_or_else(|| anyhow!("{name}: $$EOE fehlt"))?;
    let header = lines[..soe]
        .iter()
        .rev()
        .find(|l| l.contains("Date__(UT)"))
        .ok_or_else(|| anyhow!("{name}: Kopfzeile fehlt"))?;
    let cols: Vec<String> = header.split(',').map(|c| c.trim().to_string()).collect();

    let mut rows = Vec::new();
    for line in &lines[soe + 1..eoe] {
        let cells: Vec<&str> = line.split(',').map(|c| c.trim()).collect();
        let mut values = HashMap::new();
        for (c, v) in cols.iter().zip(cells.iter()) {
            if c.is_empty() {
                continue;
            }
            if let Ok(x) = v.parse::<f64>() {
                values.insert(c.clone(), x);
            }
        }
        let jd_ut = *values.get("Date_________JDUT").ok_or_else(|| anyhow!("{name}: JD fehlt in '{line}'"))?;
        rows.push(RefRow { jd_ut, values });
    }
    if rows.is_empty() {
        bail!("{name}: keine Datenzeilen");
    }
    Ok(RefFile { name: name.to_string(), site, rows })
}

/// Ergebnis einer Prüfgröße über alle Zeitpunkte und Standorte.
#[derive(Clone, Debug)]
pub struct Check {
    pub name: &'static str,
    pub unit: &'static str,
    pub tolerance: f64,
    pub max_dev: f64,
    pub rms_dev: f64,
    pub n: usize,
    pub worst: String,
    /// Soll und Ist am Ort der größten Abweichung.
    pub worst_ref: f64,
    pub worst_own: f64,
}

impl Check {
    pub fn pass(&self) -> bool {
        self.n > 0 && self.max_dev <= self.tolerance
    }
}

#[derive(Clone, Debug)]
pub struct Group {
    pub title: String,
    pub epochs: usize,
    pub sites: usize,
    pub checks: Vec<Check>,
    /// Bindend für die Abnahme (Messreihe) oder nur berichtend (Ausweichmodell).
    pub binding: bool,
}

impl Group {
    pub fn pass(&self) -> bool {
        self.checks.iter().all(|c| c.pass())
    }
}

struct Acc {
    name: &'static str,
    unit: &'static str,
    tolerance: f64,
    max: f64,
    sum2: f64,
    n: usize,
    worst: String,
    worst_ref: f64,
    worst_own: f64,
}

impl Acc {
    fn new(name: &'static str, unit: &'static str, tolerance: f64) -> Self {
        Self { name, unit, tolerance, max: 0.0, sum2: 0.0, n: 0, worst: String::new(), worst_ref: 0.0, worst_own: 0.0 }
    }
    fn add(&mut self, dev: f64, reference: f64, own: f64, at: &str) {
        let d = dev.abs();
        self.sum2 += d * d;
        self.n += 1;
        if d >= self.max {
            self.max = d;
            self.worst = at.to_string();
            self.worst_ref = reference;
            self.worst_own = own;
        }
    }
    fn finish(self) -> Check {
        Check {
            name: self.name,
            unit: self.unit,
            tolerance: self.tolerance,
            max_dev: self.max,
            rms_dev: if self.n > 0 { (self.sum2 / self.n as f64).sqrt() } else { 0.0 },
            n: self.n,
            worst: self.worst,
            worst_ref: self.worst_ref,
            worst_own: self.worst_own,
        }
    }
}

fn sep_arcsec(lon1: f64, lat1: f64, lon2: f64, lat2: f64) -> f64 {
    angle(from_lon_lat_deg(lon1, lat1), from_lon_lat_deg(lon2, lat2)) / ARCSEC
}

/// Toleranzen der Abnahme (siehe README, Abschnitt Prüfung).
pub struct Tolerances {
    pub position_arcsec: f64,
    pub horizon_arcsec: f64,
    pub distance_km: f64,
    pub illum_pp: f64,
    pub diameter_arcsec: f64,
    pub surface_deg: f64,
    pub axis_deg: f64,
    pub angle_deg: f64,
}

impl Tolerances {
    pub const SPEC: Tolerances = Tolerances {
        position_arcsec: 1.0,
        horizon_arcsec: 1.0,
        distance_km: 1.0,
        illum_pp: 0.01,
        diameter_arcsec: 0.01,
        surface_deg: 0.01,
        axis_deg: 0.01,
        angle_deg: 0.01,
    };
    /// Außerhalb der Erddrehungs-Messreihe: die Lage des Beobachters ist nur genähert.
    /// 1′ am Horizont entspricht 4 s Unsicherheit der Erddrehung.
    pub const FALLBACK: Tolerances = Tolerances {
        position_arcsec: 2.0,
        horizon_arcsec: 60.0,
        distance_km: 2.0,
        illum_pp: 0.01,
        diameter_arcsec: 0.01,
        surface_deg: 0.01,
        axis_deg: 0.01,
        angle_deg: 0.01,
    };
}

pub fn check_files(eph: &Ephemeris, title: &str, files: &[RefFile], tol: &Tolerances, binding: bool) -> Result<Group> {
    let mut a_radec = Acc::new("Position ICRF (RA/Dec)", "″", tol.position_arcsec);
    let mut a_app = Acc::new("Position des Datums (RA/Dec)", "″", tol.position_arcsec);
    let mut a_azel = Acc::new("Azimut/Höhe ohne Refraktion", "″", tol.horizon_arcsec);
    let mut a_dist = Acc::new("Entfernung", "km", tol.distance_km);
    let mut a_illum = Acc::new("Beleuchteter Anteil", "%-Pkt", tol.illum_pp);
    let mut a_diam = Acc::new("Scheinbarer Durchmesser", "″", tol.diameter_arcsec);
    let mut a_liblon = Acc::new("Libration Länge", "°", tol.surface_deg);
    let mut a_liblat = Acc::new("Libration Breite", "°", tol.surface_deg);
    let mut a_sunlon = Acc::new("Sonnenstand Länge", "°", tol.surface_deg);
    let mut a_sunlat = Acc::new("Sonnenstand Breite", "°", tol.surface_deg);
    let mut a_np = Acc::new("Achslage (Positionswinkel)", "°", tol.axis_deg);
    let mut a_sot = Acc::new("Elongation Sonne–Mond", "°", tol.angle_deg);
    let mut a_sto = Acc::new("Phasenwinkel", "°", tol.angle_deg);

    let mut epochs = 0;
    for f in files {
        epochs = epochs.max(f.rows.len());
        for row in &f.rows {
            let epoch = time::epoch_from_civil_jd(row.jd_ut);
            let sc = Scene::compute(eph, f.site, epoch)
                .with_context(|| format!("{} JD {}", f.name, row.jd_ut))?;
            if binding && sc.earth_rotation != EarthRotation::Kernel {
                bail!("{}: JD {} liegt außerhalb der Erddrehungs-Messreihe", f.name, row.jd_ut);
            }
            let at = format!("{} {}", f.name, time::format_civil(row.jd_ut));
            let g = |k: &str| -> Result<f64> {
                row.values.get(k).copied().ok_or_else(|| anyhow!("{}: Spalte '{k}' fehlt", f.name))
            };
            let (ra, dec) = (g("R.A.___(ICRF)")?, g("DEC____(ICRF)")?);
            a_radec.add(sep_arcsec(ra, dec, sc.ra_icrf_deg, sc.dec_icrf_deg), dec, sc.dec_icrf_deg, &at);
            let (ra, dec) = (g("R.A.__(a-app)")?, g("DEC___(a-app)")?);
            a_app.add(sep_arcsec(ra, dec, sc.ra_apparent_deg, sc.dec_apparent_deg), dec, sc.dec_apparent_deg, &at);
            let (az, el) = (g("Azimuth_(a-app)")?, g("Elevation_(a-app)")?);
            a_azel.add(sep_arcsec(az, el, sc.azimuth_deg, sc.altitude_deg), el, sc.altitude_deg, &at);
            let d = g("delta")?;
            a_dist.add(d - sc.distance_km, d, sc.distance_km, &at);
            let il = g("Illu%")?;
            a_illum.add(il - sc.illuminated_fraction * 100.0, il, sc.illuminated_fraction * 100.0, &at);
            let ad = g("Ang-diam")?;
            a_diam.add(ad - sc.angular_diameter_arcsec, ad, sc.angular_diameter_arcsec, &at);
            let (ol, ob) = (g("ObsSub-LON")?, g("ObsSub-LAT")?);
            a_liblon.add(wrap180(ol - sc.libration_lon_deg), wrap180(ol), sc.libration_lon_deg, &at);
            a_liblat.add(ob - sc.libration_lat_deg, ob, sc.libration_lat_deg, &at);
            let (sl, sb) = (g("SunSub-LON")?, g("SunSub-LAT")?);
            a_sunlon.add(wrap180(sl - sc.subsolar_lon_deg), wrap180(sl), sc.subsolar_lon_deg, &at);
            a_sunlat.add(sb - sc.subsolar_lat_deg, sb, sc.subsolar_lat_deg, &at);
            let np = g("NP.ang")?;
            a_np.add(wrap180(np - sc.axis_position_angle_deg), np, sc.axis_position_angle_deg, &at);
            let sot = g("S-O-T")?;
            a_sot.add(sot - sc.elongation_deg, sot, sc.elongation_deg, &at);
            // Horizons: "phi" ist der wahre Phasenwinkel; "S-T-O" enthält zusätzlich die
            // Aberration auf dem Weg zum Beobachter und ist deshalb nicht die Vergleichsgröße.
            let sto = g("phi")?;
            a_sto.add(sto - sc.phase_angle_deg, sto, sc.phase_angle_deg, &at);
        }
    }
    Ok(Group {
        title: title.to_string(),
        epochs,
        sites: files.len(),
        binding,
        checks: vec![a_radec, a_app, a_azel, a_dist, a_illum, a_diam, a_liblon, a_liblat, a_sunlon, a_sunlat, a_np, a_sot, a_sto]
            .into_iter()
            .map(Acc::finish)
            .collect(),
    })
}

/// Liest alle Referenzdateien eines Ordners und prüft sie.
pub fn run(eph: &Ephemeris, ref_dir: &Path) -> Result<Vec<Group>> {
    let mut main = Vec::new();
    let mut fall = Vec::new();
    let mut names: Vec<_> = std::fs::read_dir(ref_dir)
        .with_context(|| format!("Referenzordner {} nicht lesbar", ref_dir.display()))?
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "txt"))
        .collect();
    names.sort();
    for p in names {
        let stem = p.file_stem().unwrap().to_string_lossy().to_string();
        let text = std::fs::read_to_string(&p)?;
        let rf = parse_horizons(&stem, &text)?;
        if stem.ends_with("_main") {
            main.push(rf);
        } else if stem.ends_with("_fall") {
            fall.push(rf);
        }
    }
    if main.is_empty() {
        bail!("keine Referenzdateien *_main.txt in {}", ref_dir.display());
    }
    let mut groups = vec![check_files(
        eph,
        "JPL Horizons, 2001–2026, gemessene Erddrehung",
        &main,
        &Tolerances::SPEC,
        true,
    )?];
    if !fall.is_empty() {
        groups.push(check_files(
            eph,
            "JPL Horizons, 1905–2031, Erddrehung aus Ausweichmodell",
            &fall,
            &Tolerances::FALLBACK,
            false,
        )?);
    }
    Ok(groups)
}

pub fn format_report(groups: &[Group]) -> String {
    let mut s = String::new();
    for g in groups {
        s.push_str(&format!(
            "\n{} — {} Zeitpunkte × {} Standorte{}\n",
            g.title,
            g.epochs,
            g.sites,
            if g.binding { "" } else { "  (berichtend, Kategorie N)" }
        ));
        s.push_str(&format!(
            "  {:<30} {:>14} {:>14} {:>11} {:>11} {:>9}  {}\n",
            "Größe", "Soll", "Ist", "max. Abw.", "Toleranz", "Einheit", "Urteil"
        ));
        for c in &g.checks {
            s.push_str(&format!(
                "  {:<30} {:>14.6} {:>14.6} {:>11.5} {:>11.5} {:>9}  {}\n",
                c.name,
                c.worst_ref,
                c.worst_own,
                c.max_dev,
                c.tolerance,
                c.unit,
                if c.pass() { "besteht" } else { "VERFEHLT" }
            ));
        }
        s.push_str("  Soll und Ist stehen jeweils für den Fall mit der größten Abweichung; bei Positionen ist die Deklination bzw. Höhe gezeigt.\n");
    }
    s
}
