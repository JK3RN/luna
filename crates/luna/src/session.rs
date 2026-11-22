//! Gemeinsamer Unterbau für Fenster und Kommandozeile: Daten, Manifest, Ephemeride,
//! Einstellungen und der Zustand, den der Mensch verändert.

use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{anyhow, bail, Context, Result};
use luna_core::{time, Ephemeris, Scene, Site};
use luna_data::{DataPaths, Manifest};
use luna_render::{photometry, Camera, FrameParams, RenderMode, UpMode, View};
use luna_ui::{DiscOverlay, HudModel, ImageRow};

/// Vorgabe, solange weder Konfigurationsdatei noch Schalter einen Standort nennen.
const DEFAULT_SITE: (f64, f64, f64, &str) = (52.52, 13.405, 34.0, "Berlin");

#[derive(Clone, Debug)]
pub struct Settings {
    pub site: Site,
    pub site_name: String,
    /// Woher der Standort stammt: "Vorgabe", "Konfiguration" oder "Schalter".
    pub site_origin: &'static str,
    /// Startzeit als bürgerliches Julianisches Datum; `None` = jetzt.
    pub at: Option<f64>,
    pub ev: Option<f64>,
    pub view: View,
    pub mode: RenderMode,
    pub size: (u32, u32),
    pub scale: f32,
    pub hud: bool,
    pub command: Command,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Command {
    Window,
    Render(PathBuf),
    Verify,
    Bench,
    Info,
    Help,
}

pub const HELP: &str = "\
LUNA — Erstes Licht (Prototyp, Stufe 0)

Aufruf:  luna [Schalter]

Standort und Zeit
  --lat <Grad>        geodätische Breite, Nord positiv
  --lon <Grad>        Länge, Ost positiv
  --height <m>        Höhe über dem Ellipsoid
  --at <Zeit>         Zeitpunkt in UTC: 2026-11-24T18:30:00, 2026-11-24 oder JD2461369.27
                      (vor 1972 gilt die Angabe als UT1)

Ansicht
  --zoom <Faktor>     Vergrößerung (1 = Vollscheibe)
  --up zenit|himmelsnord|mondnord
  --yaw <Grad> --pitch <Grad>   freie Drehung um die Kugel
  --ev <Lichtwert>    feste Belichtung (EV bei ISO 100); ohne Angabe Automatik
  --mode voll|ohne-schatten|glatt

Ausgaben ohne Fenster
  --render <bild.png> ein Bild schreiben; mit --size BxH, --hud, --scale <Faktor>
  --verify            Selbstprüfung: Daten, Rechenkette, Kartenlage, Bild
  --bench             Leistungsmessung, Ergebnis als JSON
  --info              Messwerte als Text

Standort dauerhaft festlegen: ~/.config/luna/config.toml mit den Zeilen
  lat = 52.52
  lon = 13.405
  height = 34
  name = \"Berlin\"
";

fn config_path() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))?;
    Some(base.join("luna/config.toml"))
}

fn read_config(s: &mut Settings) -> Result<()> {
    let Some(path) = config_path() else { return Ok(()) };
    let Ok(text) = std::fs::read_to_string(&path) else { return Ok(()) };
    let (mut lat, mut lon, mut height) = (None, None, 0.0);
    for (n, line) in text.lines().enumerate() {
        let line = line.split('#').next().unwrap_or("").trim();
        if line.is_empty() {
            continue;
        }
        let (k, v) = line
            .split_once('=')
            .ok_or_else(|| anyhow!("{}: Zeile {} hat kein '='", path.display(), n + 1))?;
        let (k, v) = (k.trim(), v.trim().trim_matches('"'));
        let num = || {
            v.parse::<f64>()
                .ok()
                .filter(|x| x.is_finite())
                .ok_or_else(|| anyhow!("{}: '{k}' ist keine Zahl: '{v}'", path.display()))
        };
        match k {
            "lat" => lat = Some(num()?),
            "lon" => lon = Some(num()?),
            "height" => height = num()?,
            "name" => s.site_name = v.to_string(),
            _ => bail!("{}: unbekannter Eintrag '{k}'", path.display()),
        }
    }
    if let (Some(lat), Some(lon)) = (lat, lon) {
        s.site = Site::new(lat, lon, height).with_context(|| format!("{}", path.display()))?;
        s.site_origin = "Konfiguration";
    } else if lat.is_some() || lon.is_some() {
        bail!("{}: 'lat' und 'lon' müssen beide angegeben sein", path.display());
    }
    Ok(())
}

impl Settings {
    pub fn from_args(args: impl Iterator<Item = String>) -> Result<Self> {
        let mut s = Settings {
            site: Site::new(DEFAULT_SITE.0, DEFAULT_SITE.1, DEFAULT_SITE.2)?,
            site_name: DEFAULT_SITE.3.to_string(),
            site_origin: "Vorgabe",
            at: None,
            ev: None,
            view: View::default(),
            mode: RenderMode::Full,
            size: (1600, 1600),
            scale: 1.0,
            hud: false,
            command: Command::Window,
        };
        read_config(&mut s)?;

        let (mut lat, mut lon, mut height) = (None, None, None);
        let mut args = args.peekable();
        while let Some(a) = args.next() {
            let mut value = |name: &str| args.next().ok_or_else(|| anyhow!("{name} braucht einen Wert"));
            let num = |name: &str, v: String| {
                v.parse::<f64>().ok().filter(|x| x.is_finite()).ok_or_else(|| anyhow!("{name}: '{v}' ist keine Zahl"))
            };
            match a.as_str() {
                "-h" | "--help" => s.command = Command::Help,
                "--verify" => s.command = Command::Verify,
                "--bench" => s.command = Command::Bench,
                "--info" => s.command = Command::Info,
                "--hud" => s.hud = true,
                "--render" => s.command = Command::Render(PathBuf::from(value("--render")?)),
                "--lat" => lat = Some(num("--lat", value("--lat")?)?),
                "--lon" => lon = Some(num("--lon", value("--lon")?)?),
                "--height" => height = Some(num("--height", value("--height")?)?),
                "--at" => s.at = Some(time::parse_civil(&value("--at")?).map_err(|e| anyhow!("--at: {e}"))?),
                "--ev" => s.ev = Some(num("--ev", value("--ev")?)?),
                "--zoom" => s.view.zoom = num("--zoom", value("--zoom")?)?.clamp(ZOOM_MIN, ZOOM_MAX),
                "--yaw" => s.view.yaw = num("--yaw", value("--yaw")?)?.to_radians(),
                "--pitch" => s.view.pitch = num("--pitch", value("--pitch")?)?.clamp(-89.0, 89.0).to_radians(),
                "--scale" => s.scale = num("--scale", value("--scale")?)?.clamp(0.5, 4.0) as f32,
                "--up" => {
                    s.view.up = match value("--up")?.as_str() {
                        "zenit" => UpMode::Zenith,
                        "himmelsnord" => UpMode::CelestialNorth,
                        "mondnord" => UpMode::LunarNorth,
                        o => bail!("--up: '{o}' unbekannt (zenit, himmelsnord, mondnord)"),
                    }
                }
                "--mode" => {
                    s.mode = match value("--mode")?.as_str() {
                        "voll" => RenderMode::Full,
                        "ohne-schatten" => RenderMode::NoShadows,
                        "glatt" => RenderMode::Smooth,
                        o => bail!("--mode: '{o}' unbekannt (voll, ohne-schatten, glatt)"),
                    }
                }
                "--size" => {
                    let v = value("--size")?;
                    let (w, h) = v.split_once(['x', 'X']).ok_or_else(|| anyhow!("--size: erwartet BxH, etwa 2560x1440"))?;
                    let w: u32 = w.parse().map_err(|_| anyhow!("--size: Breite '{w}' nicht lesbar"))?;
                    let h: u32 = h.parse().map_err(|_| anyhow!("--size: Höhe '{h}' nicht lesbar"))?;
                    if !(64..=8192).contains(&w) || !(64..=8192).contains(&h) {
                        bail!("--size: Breite und Höhe müssen zwischen 64 und 8192 liegen");
                    }
                    s.size = (w, h);
                }
                other => bail!("unbekannter Schalter '{other}' (siehe luna --help)"),
            }
        }
        if lat.is_some() || lon.is_some() || height.is_some() {
            let site = Site::new(
                lat.unwrap_or(s.site.lat_deg),
                lon.unwrap_or(s.site.lon_deg),
                height.unwrap_or(if lat.is_some() || lon.is_some() { 0.0 } else { s.site.height_m }),
            )?;
            if lat.is_some() || lon.is_some() {
                s.site_name = String::new();
            }
            s.site = site;
            s.site_origin = "Schalter";
        }
        Ok(s)
    }
}

pub const ZOOM_MIN: f64 = 0.25;
pub const ZOOM_MAX: f64 = 16.0;

/// Bürgerliches Julianisches Datum der Systemuhr.
pub fn now_jd() -> f64 {
    let d = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default();
    2_440_587.5 + d.as_secs_f64() / 86400.0
}

/// Daten, Manifest und Ephemeride: einmal geladen, dann nur gelesen.
pub struct Session {
    pub paths: DataPaths,
    pub manifest: Manifest,
    pub eph: Ephemeris,
    /// Zulässiger Zeitraum als bürgerliches Julianisches Datum.
    pub jd_range: (f64, f64),
    pub checked_bytes: u64,
}

impl Session {
    pub fn load() -> Result<Self> {
        let paths = DataPaths::locate()?;
        let manifest = Manifest::load(&paths)?;
        let checked_bytes = manifest.verify(&paths).context("Datenprüfung beim Start")?;
        let eph = Ephemeris::load(&paths.raw())?;
        let (a, b) = eph.domain();
        Ok(Self { paths, manifest, eph, jd_range: (time::civil_jd(a), time::civil_jd(b)), checked_bytes })
    }

    pub fn clamp_jd(&self, jd: f64) -> f64 {
        jd.clamp(self.jd_range.0, self.jd_range.1)
    }

    pub fn scene(&self, site: Site, jd: f64) -> Result<Scene> {
        Scene::compute(&self.eph, site, time::epoch_from_civil_jd(jd))
    }
}

/// Zeitraffer-Stufen: simulierte Sekunden je echter Sekunde.
pub const RATES: &[(f64, &str)] = &[
    (-30.0 * 86400.0, "ZURÜCK 30 d/s"),
    (-7.0 * 86400.0, "ZURÜCK 7 d/s"),
    (-86400.0, "ZURÜCK 1 d/s"),
    (-6.0 * 3600.0, "ZURÜCK 6 h/s"),
    (-3600.0, "ZURÜCK 1 h/s"),
    (-600.0, "ZURÜCK 10 min/s"),
    (-60.0, "ZURÜCK 1 min/s"),
    (1.0, "ECHTZEIT"),
    (60.0, "VOR 1 min/s"),
    (600.0, "VOR 10 min/s"),
    (3600.0, "VOR 1 h/s"),
    (6.0 * 3600.0, "VOR 6 h/s"),
    (86400.0, "VOR 1 d/s"),
    (7.0 * 86400.0, "VOR 7 d/s"),
    (30.0 * 86400.0, "VOR 30 d/s"),
];
pub const RATE_REALTIME: usize = 7;

/// Alles, was der Mensch verstellt.
#[derive(Clone, Debug)]
pub struct Controls {
    pub jd: f64,
    pub rate: usize,
    pub paused: bool,
    /// Läuft die Zeit an der Systemuhr entlang? (Sonst ist sie verstellt.)
    pub follows_clock: bool,
    pub view: View,
    pub ev_manual: Option<f64>,
    pub mode: RenderMode,
    pub show_help: bool,
}

impl Controls {
    pub fn new(s: &Settings) -> Self {
        Self {
            jd: s.at.unwrap_or_else(now_jd),
            rate: RATE_REALTIME,
            paused: s.at.is_some(),
            follows_clock: s.at.is_none(),
            view: s.view,
            ev_manual: s.ev,
            mode: s.mode,
            show_help: false,
        }
    }

    pub fn animating(&self) -> bool {
        !self.paused && self.rate != RATE_REALTIME
    }

    pub fn rate_text(&self) -> String {
        if self.paused {
            "ANGEHALTEN".to_string()
        } else if self.rate == RATE_REALTIME && self.follows_clock {
            "ECHTZEIT · JETZT".to_string()
        } else {
            RATES[self.rate].1.to_string()
        }
    }
}

/// Ein fertig berechnetes Bild: Szene, Kamera, Zahlen für die Grafikkarte.
pub struct Frame {
    pub scene: Scene,
    pub camera: Camera,
    pub params: FrameParams,
    pub ev: f64,
    pub ev_auto: bool,
}

impl Frame {
    pub fn build(session: &Session, site: Site, c: &Controls, width: u32, height: u32) -> Result<Self> {
        let scene = session.scene(site, c.jd)?;
        let camera = Camera::from_scene(&scene, &c.view);
        let alpha = luna_core::math::angle(scene.sun_from_moon_km, camera.pos_km).to_degrees();
        let sun_au = scene.sun_distance_km / luna_core::eph::AU_KM;
        let (ev, ev_auto) = match c.ev_manual {
            Some(ev) => (ev, false),
            None => (photometry::auto_ev100(alpha, sun_au), true),
        };
        let params = FrameParams::build(
            &scene,
            &camera,
            width,
            height,
            ev,
            session.manifest.forge.albedo_mean_linear,
            session.manifest.forge.horizon_max_angle_deg,
            c.mode as u32,
        );
        Ok(Self { scene, camera, params, ev, ev_auto })
    }
}

fn site_text(s: &Settings) -> String {
    let site = &s.site;
    let ns = if site.lat_deg >= 0.0 { "N" } else { "S" };
    let ew = if site.lon_deg >= 0.0 { "O" } else { "W" };
    let name = if s.site_name.is_empty() { String::new() } else { format!("{}  ", s.site_name.to_uppercase()) };
    format!(
        "{name}{:.4}° {ns}  {:.4}° {ew}  {:.0} m  ({})",
        site.lat_deg.abs(),
        site.lon_deg.abs(),
        site.height_m,
        s.site_origin
    )
}

/// Baut die Anzeige. `ppp`: Pixel je Punkt; `status`: Fußzeile.
#[allow(clippy::too_many_arguments)]
pub fn hud_model(
    session: &Session,
    settings: &Settings,
    c: &Controls,
    f: &Frame,
    width: u32,
    height: u32,
    ppp: f32,
    status: String,
    extra_notices: &[String],
) -> HudModel {
    let sc = &f.scene;
    let forge = &session.manifest.forge;
    let mut notices = Vec::new();
    if c.view.is_free() {
        notices.push("FREIE ANSICHT · nicht der Blick vom Standort; Messwerte gelten für den Standort".to_string());
    } else if sc.altitude_deg < 0.0 {
        notices.push(format!("MOND UNTER DEM HORIZONT ({:+.1}°) · Ansicht durch die Erde hindurch", sc.altitude_deg));
    }
    match sc.earth_rotation {
        luna_core::EarthRotation::Kernel => {}
        other => notices.push(format!("ERDDREHUNG: {} · Lage am Himmel nur genähert (N)", other.label().to_uppercase())),
    }
    notices.extend(extra_notices.iter().cloned());

    let km_px = f.camera.km_per_pixel(width, height);
    let finest = forge.albedo_texel_km.min(forge.height_texel_km);
    let (shadow_value, shadow_kind) = match c.mode {
        RenderMode::Full => ("Horizontkarten", "B"),
        _ => ("aus", "K"),
    };
    let relief_value = match c.mode {
        RenderMode::Smooth | RenderMode::Gray => "aus".to_string(),
        _ => format!("LOLA {:.1} km", forge.height_texel_km),
    };
    let image_rows = vec![
        ImageRow {
            label: "RELIEF",
            value: relief_value,
            kind: "M",
            detail: "M (gemessen)  ·  LRO/LOLA-Höhenmodell, 16 Pixel je Grad, aus dem CGI Moon Kit (NASA's Scientific Visualization Studio)",
        },
        ImageRow {
            label: "FARBE",
            value: format!("LROC WAC {:.1} km", forge.albedo_texel_km),
            kind: "K",
            detail: "K (künstlerisch)  ·  Farbkarte laut NASA für Ästhetik optimiert, nicht für Wissenschaft; auf mittlere Albedo 0,12 skaliert",
        },
        ImageRow {
            label: "SCHATTEN",
            value: shadow_value.to_string(),
            kind: shadow_kind,
            detail: "B auf M  ·  Horizontwinkel in 8 Richtungen aus dem Höhenmodell; Halbschatten aus dem Sonnendurchmesser",
        },
        ImageRow {
            label: "STREUUNG",
            value: "Lommel-Seeliger".to_string(),
            kind: "N",
            detail: "N (Näherung)  ·  Lommel-Seeliger mit Phasenfunktion aus der Helligkeitskurve nach Allen; einige Prozent Abweichung, ab 150° gehalten",
        },
        ImageRow {
            label: "BELICHTUNG",
            value: format!("EV {:.1}{}", f.ev, if f.ev_auto { " AUTO" } else { " FEST" }),
            kind: "K",
            detail: "K (Gestaltung)  ·  Lichtwert bei ISO 100 wie an einer Kamera; die Helligkeit des Bildes ist eine Belichtungsentscheidung",
        },
        ImageRow {
            label: "MASSSTAB",
            value: if km_px < finest {
                format!("{:.2} km/px > Daten", km_px)
            } else {
                format!("{:.2} km/px", km_px)
            },
            kind: "B",
            detail: "B (berechnet)  ·  Kantenlänge eines Bildpunkts in Scheibenmitte. Feiner als die Karten wird das Bild nicht schärfer: LUNA erfindet kein Detail",
        },
    ];

    // Marken am Scheibenrand.
    let p = ppp as f64;
    let disc = f.camera.project([0.0; 3], width, height).map(|(cx, cy)| {
        let dir = |v: luna_core::math::V3| -> Option<[f32; 2]> {
            let tip = luna_core::math::scale(luna_core::math::unit(v), luna_core::MOON_RADIUS_KM);
            let (x, y) = f.camera.project(tip, width, height)?;
            let (dx, dy) = (x - cx, y - cy);
            let len = (dx * dx + dy * dy).sqrt();
            (len > 0.15 * f.camera.disc_radius_px(width, height)).then(|| [(dx / len) as f32, (dy / len) as f32])
        };
        DiscOverlay {
            center: [(cx / p) as f32, (cy / p) as f32],
            radius: (f.camera.disc_radius_px(width, height) / p) as f32,
            lunar_north: dir(luna_core::math::mul_mv(&sc.moon_me_to_j2000, [0.0, 0.0, 1.0])),
            zenith: dir(sc.zenith),
        }
    });

    HudModel {
        time_text: format!("{} UTC", time::format_civil(sc.civil_jd)),
        rate_text: c.rate_text(),
        site_text: site_text(settings),
        phase_text: format!(
            "{}  ·  AZIMUT {:.1}°  ·  SONNE {:+.1}°",
            sc.phase_name().to_uppercase(),
            sc.azimuth_deg,
            sc.sun_altitude_deg
        ),
        view_text: format!(
            "{}  ·  {}  ·  ZOOM {:.2}×{}",
            if c.view.is_free() { "FREIE ANSICHT" } else { "BLICK VOM STANDORT" },
            c.view.up.label(),
            c.view.zoom,
            if c.mode == RenderMode::Full { String::new() } else { format!("  ·  BILD {}", c.mode.label()) }
        ),
        readings: sc.readings(),
        image_rows,
        notices,
        status_text: status,
        show_help: c.show_help,
        disc,
    }
}
