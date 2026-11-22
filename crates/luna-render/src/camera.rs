//! Kamera: aus Szenenzustand und Ansicht werden die Zahlen für die Grafikkarte.
//!
//! Alle Positionen bleiben hier in 64 Bit. Erst die fertigen, kamerabezogenen Größen
//! werden auf 32 Bit verkürzt.

use luna_core::math::*;
use luna_core::{Scene, MOON_RADIUS_KM, SUN_RADIUS_KM};

use crate::photometry;

/// Bildwinkel über die kürzere Fensterseite bei Zoom 1, in Grad. Der Mond misst
/// je nach Entfernung 0,49° bis 0,56°; seine Größe im Bild ändert sich also wirklich.
pub const BASE_FOV_DEG: f64 = 0.70;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UpMode {
    /// Zenit oben: so steht der Mond am Himmel.
    Zenith,
    /// Himmelsnordpol oben: wie in Atlanten und Jahrbüchern.
    CelestialNorth,
    /// Mondnordpol oben: wie auf Mondkarten.
    LunarNorth,
}

impl UpMode {
    pub fn label(self) -> &'static str {
        match self {
            UpMode::Zenith => "ZENIT OBEN",
            UpMode::CelestialNorth => "HIMMELSNORD OBEN",
            UpMode::LunarNorth => "MONDNORD OBEN",
        }
    }
    pub fn next(self) -> Self {
        match self {
            UpMode::Zenith => UpMode::CelestialNorth,
            UpMode::CelestialNorth => UpMode::LunarNorth,
            UpMode::LunarNorth => UpMode::Zenith,
        }
    }
}

/// Was der Mensch einstellt.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct View {
    pub zoom: f64,
    /// Freie Drehung um die Mondkugel (Bogenmaß). Beide null: Blick vom Standort.
    pub yaw: f64,
    pub pitch: f64,
    pub up: UpMode,
}

impl Default for View {
    fn default() -> Self {
        Self { zoom: 1.0, yaw: 0.0, pitch: 0.0, up: UpMode::Zenith }
    }
}

impl View {
    pub fn is_free(&self) -> bool {
        self.yaw != 0.0 || self.pitch != 0.0
    }
}

/// Kamera im Raum, Achsen J2000, Ursprung Mondmitte.
#[derive(Clone, Copy, Debug)]
pub struct Camera {
    pub pos_km: V3,
    pub right: V3,
    pub up: V3,
    pub fwd: V3,
    /// Bildwinkel über die kürzere Fensterseite (Bogenmaß).
    pub fov_min: f64,
}

/// Dreht `v` um die Einheitsachse `axis` (Rodrigues).
fn rotate(v: V3, axis: V3, angle: f64) -> V3 {
    let (s, c) = angle.sin_cos();
    add(add(scale(v, c), scale(cross(axis, v), s)), scale(axis, dot(axis, v) * (1.0 - c)))
}

impl Camera {
    pub fn from_scene(scene: &Scene, view: &View) -> Self {
        let fwd0 = unit(scene.moon_from_obs_km);
        let reference = match view.up {
            UpMode::Zenith => scene.zenith,
            UpMode::CelestialNorth => scene.celestial_pole,
            UpMode::LunarNorth => mul_mv(&scene.moon_me_to_j2000, [0.0, 0.0, 1.0]),
        };
        let mut up0 = sub(reference, scale(fwd0, dot(reference, fwd0)));
        if norm(up0) < 1e-6 {
            // Mond genau im Zenit bzw. Blick entlang der Achse: Ersatzrichtung.
            let alt = scene.north;
            up0 = sub(alt, scale(fwd0, dot(alt, fwd0)));
        }
        let up0 = unit(up0);
        let right0 = cross(fwd0, up0);
        let pos0 = scale(scene.moon_from_obs_km, -1.0);

        // Freie Drehung: erst Neigen um die Rechts-Achse, dann Schwenken um die Hoch-Achse.
        let turn = |v: V3| rotate(rotate(v, right0, view.pitch), up0, view.yaw);
        Camera {
            pos_km: turn(pos0),
            right: turn(right0),
            up: turn(up0),
            fwd: turn(fwd0),
            fov_min: (BASE_FOV_DEG * DEG) / view.zoom,
        }
    }

    fn tan_xy(&self, width: u32, height: u32) -> (f64, f64) {
        let m = width.min(height) as f64;
        let t = (self.fov_min / 2.0).tan();
        (t * width as f64 / m, t * height as f64 / m)
    }

    /// Bildpunkt (Pixelkoordinaten, Ursprung oben links) eines Punktes, der relativ zur
    /// Mondmitte in J2000 gegeben ist. `None`, wenn er hinter der Kamera liegt.
    pub fn project(&self, p_km: V3, width: u32, height: u32) -> Option<(f64, f64)> {
        let rel = sub(p_km, self.pos_km);
        let z = dot(rel, self.fwd);
        if z <= 0.0 {
            return None;
        }
        let (tx, ty) = self.tan_xy(width, height);
        let nx = dot(rel, self.right) / (z * tx);
        let ny = dot(rel, self.up) / (z * ty);
        Some(((nx * 0.5 + 0.5) * width as f64, (0.5 - ny * 0.5) * height as f64))
    }

    /// Ob ein Oberflächenpunkt (relativ zur Mondmitte, J2000) der Kamera zugewandt ist.
    pub fn faces_camera(&self, p_km: V3) -> bool {
        dot(unit(p_km), unit(sub(self.pos_km, p_km))) > 0.0
    }

    /// Kantenlänge eines Bildpunktes auf der Mondoberfläche in Scheibenmitte, in km.
    pub fn km_per_pixel(&self, width: u32, height: u32) -> f64 {
        let d = norm(self.pos_km) - MOON_RADIUS_KM;
        d * 2.0 * (self.fov_min / 2.0).tan() / width.min(height) as f64
    }

    /// Halbmesser der Mondscheibe im Bild, in Pixeln.
    pub fn disc_radius_px(&self, width: u32, height: u32) -> f64 {
        let ang = (MOON_RADIUS_KM / norm(self.pos_km)).asin();
        ang.tan() / (self.fov_min / 2.0).tan() * width.min(height) as f64 / 2.0
    }
}

/// Der fertige Zahlensatz für ein Bild. 20 × vec4, passend zum Uniform-Block im Shader.
#[repr(C)]
#[derive(Clone, Copy, Debug, bytemuck::Pod, bytemuck::Zeroable)]
pub struct FrameParams {
    /// Kameraachsen (rechts, oben, vorwärts) im Kartensystem des Mondes.
    pub cam_x: [f32; 4],
    pub cam_y: [f32; 4],
    pub cam_z: [f32; 4],
    /// Mondmitte in Kamerakoordinaten, Einheit Mondradien.
    pub center_cam: [f32; 4],
    /// Richtung zur Sonne im Kartensystem; w = scheinbarer Halbmesser der Sonne (Bogenmaß).
    pub sun_dir: [f32; 4],
    /// tan(halber Bildwinkel) in x und y; Bildgröße in Pixeln.
    pub proj: [f32; 4],
    /// x: Anzeigefaktor (Strahldichtefaktor → Anzeigewert), y: größter Horizontwinkel (Bogenmaß),
    /// z: Darstellungsart, w: Skalierung der Farbkarte auf die Normalalbedo.
    pub photo: [f32; 4],
}

impl FrameParams {
    /// `ev100`: Lichtwert der Belichtung. `albedo_mean_linear`, `horizon_max_angle_deg`: aus dem Manifest.
    #[allow(clippy::too_many_arguments)]
    pub fn build(
        scene: &Scene,
        camera: &Camera,
        width: u32,
        height: u32,
        ev100: f64,
        albedo_mean_linear: f64,
        horizon_max_angle_deg: f64,
        mode: u32,
    ) -> Self {
        let to_me = transpose(&scene.moon_me_to_j2000);
        let f3 = |v: V3, w: f64| [v[0] as f32, v[1] as f32, v[2] as f32, w as f32];
        let center = scale(camera.pos_km, -1.0 / MOON_RADIUS_KM);
        let center_cam = [dot(center, camera.right), dot(center, camera.up), dot(center, camera.fwd)];
        let sun_radius = (SUN_RADIUS_KM / scene.sun_distance_km).asin();
        let (tx, ty) = camera.tan_xy(width, height);

        // Phasenwinkel aus Sicht der Kamera (bei freier Ansicht anders als vom Standort).
        let alpha = angle(scene.sun_from_moon_km, camera.pos_km) / DEG;
        let sun_au = scene.sun_distance_km / luna_core::eph::AU_KM;
        let illuminance = photometry::SOLAR_ILLUMINANCE_LX / (sun_au * sun_au);
        // Leuchtdichte = Strahldichtefaktor · E/π; Strahldichtefaktor = Albedo · 2μ₀/(μ₀+μ) · f(α).
        let display_gain = illuminance / std::f64::consts::PI * 2.0 * photometry::surface_phase_function(alpha)
            / photometry::saturation_luminance(ev100);
        let albedo_scale = photometry::MEAN_NORMAL_ALBEDO / albedo_mean_linear;

        FrameParams {
            cam_x: f3(mul_mv(&to_me, camera.right), 0.0),
            cam_y: f3(mul_mv(&to_me, camera.up), 0.0),
            cam_z: f3(mul_mv(&to_me, camera.fwd), 0.0),
            center_cam: f3(center_cam, 0.0),
            sun_dir: f3(unit(mul_mv(&to_me, scene.sun_from_moon_km)), sun_radius),
            proj: [tx as f32, ty as f32, width as f32, height as f32],
            photo: [display_gain as f32, (horizon_max_angle_deg * DEG) as f32, mode as f32, albedo_scale as f32],
        }
    }
}
