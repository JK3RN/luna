//! Kleine 64-Bit-Vektorrechnung. Bewusst ohne Fremdtypen, damit der Kern
//! hinter einer schmalen Schnittstelle bleibt.

pub type V3 = [f64; 3];
/// Zeilenweise: `m[zeile][spalte]`.
pub type M3 = [[f64; 3]; 3];

pub const DEG: f64 = std::f64::consts::PI / 180.0;
pub const ARCSEC: f64 = DEG / 3600.0;

pub fn add(a: V3, b: V3) -> V3 {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}
pub fn sub(a: V3, b: V3) -> V3 {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
pub fn scale(a: V3, s: f64) -> V3 {
    [a[0] * s, a[1] * s, a[2] * s]
}
pub fn dot(a: V3, b: V3) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}
pub fn cross(a: V3, b: V3) -> V3 {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}
pub fn norm(a: V3) -> f64 {
    dot(a, a).sqrt()
}
pub fn unit(a: V3) -> V3 {
    scale(a, 1.0 / norm(a))
}
/// Winkel zwischen zwei Vektoren, numerisch stabil auch nahe 0 und π.
pub fn angle(a: V3, b: V3) -> f64 {
    norm(cross(a, b)).atan2(dot(a, b))
}
pub fn mul_mv(m: &M3, v: V3) -> V3 {
    [dot(m[0], v), dot(m[1], v), dot(m[2], v)]
}
pub fn transpose(m: &M3) -> M3 {
    [
        [m[0][0], m[1][0], m[2][0]],
        [m[0][1], m[1][1], m[2][1]],
        [m[0][2], m[1][2], m[2][2]],
    ]
}
pub fn mul_mm(a: &M3, b: &M3) -> M3 {
    let bt = transpose(b);
    [
        [dot(a[0], bt[0]), dot(a[0], bt[1]), dot(a[0], bt[2])],
        [dot(a[1], bt[0]), dot(a[1], bt[1]), dot(a[1], bt[2])],
        [dot(a[2], bt[0]), dot(a[2], bt[1]), dot(a[2], bt[2])],
    ]
}
pub const IDENTITY: M3 = [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];

/// Koordinatendrehung um die Achse `axis` (1, 2 oder 3) nach SPICE-Konvention `[θ]_i`:
/// dreht das Bezugssystem, nicht den Vektor.
pub fn rot_axis(axis: u8, theta: f64) -> M3 {
    let (s, c) = theta.sin_cos();
    match axis {
        1 => [[1.0, 0.0, 0.0], [0.0, c, s], [0.0, -s, c]],
        2 => [[c, 0.0, -s], [0.0, 1.0, 0.0], [s, 0.0, c]],
        3 => [[c, s, 0.0], [-s, c, 0.0], [0.0, 0.0, 1.0]],
        _ => panic!("Achse muss 1, 2 oder 3 sein"),
    }
}

/// Länge (−180°…180°] und Breite eines Richtungsvektors, in Grad.
pub fn lon_lat_deg(v: V3) -> (f64, f64) {
    let u = unit(v);
    (u[1].atan2(u[0]) / DEG, u[2].clamp(-1.0, 1.0).asin() / DEG)
}

/// Einheitsvektor aus Länge und Breite in Grad.
pub fn from_lon_lat_deg(lon: f64, lat: f64) -> V3 {
    let (sl, cl) = (lon * DEG).sin_cos();
    let (sp, cp) = (lat * DEG).sin_cos();
    [cp * cl, cp * sl, sp]
}

pub fn wrap360(d: f64) -> f64 {
    d.rem_euclid(360.0)
}
pub fn wrap180(d: f64) -> f64 {
    (d + 180.0).rem_euclid(360.0) - 180.0
}
