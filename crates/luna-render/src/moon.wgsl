// LUNA — Mondkugel.
//
// Jeder Bildpunkt schickt einen Sehstrahl gegen die Kugel. Am Treffpunkt:
//   M  Farbe (Farbkarte, als Albedo skaliert: K/N), Neigung und Horizont (aus LOLA-Höhen)
//   B  Sonnenrichtung, Blickrichtung
//   N  Streuung nach Lommel-Seeliger mit Phasenfunktion
// Gerechnet wird der Strahldichtefaktor; die Belichtung ist der letzte Schritt.

struct Frame {
    cam_x: vec4f,
    cam_y: vec4f,
    cam_z: vec4f,
    center_cam: vec4f,
    sun_dir: vec4f,
    proj: vec4f,
    photo: vec4f,
}

@group(0) @binding(0) var<uniform> frame: Frame;
@group(0) @binding(1) var map_sampler: sampler;
@group(0) @binding(2) var albedo_map: texture_2d<f32>;
@group(0) @binding(3) var relief_map: texture_2d<f32>;
@group(0) @binding(4) var horizon_a: texture_2d<f32>;
@group(0) @binding(5) var horizon_b: texture_2d<f32>;

const PI: f32 = 3.14159265358979;

@vertex
fn vs_main(@builtin(vertex_index) i: u32) -> @builtin(position) vec4f {
    // Ein Dreieck, das den ganzen Bildschirm deckt.
    let x = f32(i32(i & 1u) * 4 - 1);
    let y = f32(i32(i >> 1u) * 4 - 1);
    return vec4f(x, y, 0.0, 1.0);
}

fn decode_horizon(v: f32) -> f32 {
    let x = 2.0 * v - 1.0;
    return sign(x) * x * x * frame.photo.y;
}

// Sichtbarer Anteil der Sonnenscheibe, wenn ihr Mittelpunkt `x` Sonnenhalbmesser
// über dem Horizont steht (Fläche eines Kreisabschnitts).
fn sun_visible(x: f32) -> f32 {
    let c = clamp(x, -1.0, 1.0);
    return 0.5 + (c * sqrt(1.0 - c * c) + asin(c)) / PI;
}

fn srgb_encode(c: vec3f) -> vec3f {
    let lo = c * 12.92;
    let hi = 1.055 * pow(max(c, vec3f(0.0)), vec3f(1.0 / 2.4)) - 0.055;
    return select(hi, lo, c <= vec3f(0.0031308));
}

// Festes Rauschmuster je Bildpunkt, damit feine Verläufe in 8 Bit nicht streifen.
// Hängt nur von der Pixelposition ab: dasselbe Kommando erzeugt dasselbe Bild.
fn dither(p: vec2f) -> f32 {
    let q = fract(p * vec2f(0.06711056, 0.00583715));
    return fract(52.9829189 * fract(q.x + q.y)) - 0.5;
}

@fragment
fn fs_main(@builtin(position) pos: vec4f) -> @location(0) vec4f {
    let mode = u32(frame.photo.z + 0.5);

    // Sehstrahl in Kamerakoordinaten (x rechts, y oben, z vorwärts).
    let ndc = vec2f(pos.x / frame.proj.z * 2.0 - 1.0, 1.0 - pos.y / frame.proj.w * 2.0);
    let d = normalize(vec3f(ndc.x * frame.proj.x, ndc.y * frame.proj.y, 1.0));
    let c = frame.center_cam.xyz;

    // Abstand des Strahls von der Mondmitte. Das Kreuzprodukt bleibt in 32 Bit genau,
    // weil die große Komponente von c nur auf die kleinen Komponenten von d trifft.
    let w = cross(c, d);
    let b = length(w);
    let edge = fwidth(b);
    let coverage = clamp((1.0 - b) / max(edge, 1e-9) + 0.5, 0.0, 1.0);

    // Treffpunkt relativ zur Mondmitte (Einheitskugel), zuerst in Kameraachsen.
    // Außerhalb der Scheibe ist es der nächstgelegene Randpunkt; so bleiben die
    // Ableitungen unten überall definiert.
    let s = sqrt(max(1.0 - b * b, 0.0));
    let p_cam = normalize(cross(w, d) - s * d);
    let to_body = mat3x3f(frame.cam_x.xyz, frame.cam_y.xyz, frame.cam_z.xyz);
    let p = to_body * p_cam;
    let view = -(to_body * d);

    // Länge/Breite → Kartenkoordinaten.
    let lon = atan2(p.y, p.x);
    let lat = asin(clamp(p.z, -1.0, 1.0));
    let uv = vec2f(lon / (2.0 * PI) + 0.5, 0.5 - lat / PI);

    // Ableitungen für die Wahl der Kartenstufe; an der Naht bei ±180° die stetige Variante.
    let u_alt = fract(uv.x + 0.5) - 0.5;
    var du = vec2f(dpdx(uv.x), dpdy(uv.x));
    let du_alt = vec2f(dpdx(u_alt), dpdy(u_alt));
    if (dot(du_alt, du_alt) < dot(du, du)) {
        du = du_alt;
    }
    let ddx = vec2f(du.x, dpdx(uv.y));
    let ddy = vec2f(du.y, dpdy(uv.y));

    if (coverage <= 0.0 || dot(c, d) <= 0.0) {
        return vec4f(0.0, 0.0, 0.0, 1.0);
    }

    if (mode == 1u) {
        // Prüfbild: Länge und Breite je in 16 Bit.
        let a = clamp(uv, vec2f(0.0), vec2f(0.99999)) * 65536.0;
        let hi = floor(a / 256.0);
        let lo = floor(a - hi * 256.0);
        return vec4f(hi.x / 255.0, lo.x / 255.0, hi.y / 255.0, lo.y / 255.0);
    }

    var albedo = textureSampleGrad(albedo_map, map_sampler, uv, ddx, ddy).rgb * frame.photo.w;
    let smooth_sphere = mode == 3u || mode == 4u;
    if (mode == 4u) {
        // Prüfbild: einheitlich graue Kugel, zeigt allein das Streumodell.
        albedo = vec3f(0.12);
    }
    let slope = textureSampleGrad(relief_map, map_sampler, uv, ddx, ddy).rg;
    let ha = textureSampleGrad(horizon_a, map_sampler, uv, ddx, ddy);
    let hb = textureSampleGrad(horizon_b, map_sampler, uv, ddx, ddy);

    // Örtliche Achsen auf der Kugel.
    let east = normalize(vec3f(-p.y, p.x, 0.0) + vec3f(0.0, 1e-12, 0.0));
    let north = cross(p, east);
    var n = p;
    if (!smooth_sphere) {
        n = normalize(p - slope.x * east - slope.y * north);
    }

    let sun = frame.sun_dir.xyz;
    let mu0 = max(dot(n, sun), 0.0);
    let mu = max(dot(n, view), 0.02);

    // Schlagschatten aus den Horizontkarten, Halbschatten aus dem Sonnendurchmesser.
    var lit = 1.0;
    if (mode != 2u && !smooth_sphere) {
        let sun_el = asin(clamp(dot(p, sun), -1.0, 1.0));
        let az = atan2(dot(sun, east), dot(sun, north));
        let k = az / (PI / 4.0) + 8.0;
        let i0 = u32(floor(k)) % 8u;
        let i1 = (i0 + 1u) % 8u;
        let t = fract(k);
        var hs = array<f32, 8>(
            decode_horizon(ha.r), decode_horizon(ha.g), decode_horizon(ha.b), decode_horizon(ha.a),
            decode_horizon(hb.r), decode_horizon(hb.g), decode_horizon(hb.b), decode_horizon(hb.a));
        let horizon = mix(hs[i0], hs[i1], t);
        lit = sun_visible((sun_el - horizon) / frame.sun_dir.w);
    } else {
        // Ohne Relief: nur die Kugel selbst begrenzt das Licht.
        lit = sun_visible(asin(clamp(dot(p, sun), -1.0, 1.0)) / frame.sun_dir.w);
    }

    // Lommel-Seeliger: μ₀/(μ₀+μ). Der Faktor 2, die Phasenfunktion und die Belichtung stecken in photo.x.
    let radiance_factor = albedo * (mu0 / (mu0 + mu)) * lit;
    let display = radiance_factor * frame.photo.x;

    let encoded = srgb_encode(clamp(display, vec3f(0.0), vec3f(1.0)));
    let dithered = encoded + dither(pos.xy) / 255.0;
    return vec4f(clamp(dithered, vec3f(0.0), vec3f(1.0)) * coverage, 1.0);
}
