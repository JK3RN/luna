//! Ausgaben ohne Fenster: Bild, Selbstprüfung, Leistungsmessung, Messwerte als Text.

use std::fs::File;
use std::io::BufWriter;
use std::path::Path;
use std::time::Instant;

use anyhow::{anyhow, bail, Context, Result};
use luna_core::math::*;
use luna_core::{time, verify, Site, MOON_RADIUS_KM};
use luna_render::{Gpu, MoonRenderer, Offscreen, RenderMode, View, OFFSCREEN_FORMAT};

use crate::hudpaint::{load_palette, HudPainter};
use crate::session::{hud_model, Controls, Frame, Session, Settings};

pub fn write_png(path: &Path, width: u32, height: u32, rgba: &[u8], notes: &[(String, String)]) -> Result<()> {
    let f = File::create(path).with_context(|| format!("{} nicht schreibbar", path.display()))?;
    let mut enc = png::Encoder::new(BufWriter::new(f), width, height);
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    for (k, v) in notes {
        enc.add_itxt_chunk(k.clone(), v.clone())?;
    }
    let mut w = enc.write_header()?;
    w.write_image_data(rgba)?;
    w.finish()?;
    Ok(())
}

/// Der Bildbeleg: alles, woraus sich dasselbe Bild wieder erzeugen lässt.
pub fn receipt(session: &Session, settings: &Settings, c: &Controls, f: &Frame, size: (u32, u32)) -> Vec<(String, String)> {
    let sum = |p: &str| session.manifest.entry(p).map(|e| e.sha256.clone()).unwrap_or_default();
    vec![
        ("Software".into(), format!("LUNA {} (Erstes Licht)", env!("CARGO_PKG_VERSION"))),
        ("LUNA:Zeit".into(), format!("{} UTC (JD {:.6})", time::format_civil(c.jd), c.jd)),
        (
            "LUNA:Standort".into(),
            format!("lat {:.5} lon {:.5} hoehe {:.0} m", settings.site.lat_deg, settings.site.lon_deg, settings.site.height_m),
        ),
        (
            "LUNA:Ansicht".into(),
            format!(
                "zoom {} yaw {:.4} pitch {:.4} oben {:?} bild {:?} groesse {}x{} ev {:.3}{}",
                c.view.zoom,
                c.view.yaw.to_degrees(),
                c.view.pitch.to_degrees(),
                c.view.up,
                c.mode,
                size.0,
                size.1,
                f.ev,
                if f.ev_auto { " (Automatik)" } else { "" }
            ),
        ),
        (
            "LUNA:Kerne".into(),
            format!(
                "de440s.bsp {} | moon_pa_de440_200625.bpc {} | earth_latest_high_prec.bpc {} | Erddrehung: {}",
                sum("raw/de440s.bsp"),
                sum("raw/moon_pa_de440_200625.bpc"),
                sum("raw/earth_latest_high_prec.bpc"),
                f.scene.earth_rotation.label()
            ),
        ),
        (
            "LUNA:Karten".into(),
            format!(
                "albedo {} | relief {} | horizon_a {} | horizon_b {}",
                sum("forged/albedo.lmap"),
                sum("forged/relief.lmap"),
                sum("forged/horizon_a.lmap"),
                sum("forged/horizon_b.lmap")
            ),
        ),
        (
            "LUNA:Quellen".into(),
            "NASA's Scientific Visualization Studio (CGI Moon Kit; LRO/LOLA, LRO/LROC) · NASA/JPL (DE440, NAIF)".into(),
        ),
    ]
}

struct Stage {
    gpu: Gpu,
    moon: MoonRenderer,
}

fn stage(session: &Session) -> Result<Stage> {
    let gpu = Gpu::headless()?;
    let moon = MoonRenderer::new(&gpu, &session.paths, &session.manifest, OFFSCREEN_FORMAT)?;
    Ok(Stage { gpu, moon })
}

fn render_rgba(st: &Stage, session: &Session, site: Site, c: &Controls, size: (u32, u32)) -> Result<(Vec<u8>, Frame)> {
    let frame = Frame::build(session, site, c, size.0, size.1)?;
    let off = Offscreen::new(&st.gpu, size.0, size.1);
    off.render(&st.gpu, &st.moon, &frame.params)?;
    Ok((off.read(&st.gpu)?, frame))
}

pub fn render(session: &Session, settings: &Settings, path: &Path) -> Result<()> {
    let st = stage(session)?;
    // Ein ausdrücklich verlangter Zeitpunkt wird nie stillschweigend verschoben:
    // Liegt er außerhalb der Ephemeride, meldet die Rechnung das als Fehler.
    let c = Controls::new(settings);
    let size = settings.size;
    let frame = Frame::build(session, settings.site, &c, size.0, size.1)?;
    let off = Offscreen::new(&st.gpu, size.0, size.1);
    let rgba = if settings.hud {
        let mut hud = HudPainter::new(&st.gpu, OFFSCREEN_FORMAT);
        let ppp = settings.scale;
        let model = hud_model(session, settings, &c, &frame, size.0, size.1, ppp, format!("{}  ·  Bild ohne Fenster", st.gpu.info.name), &[]);
        let palette = load_palette();
        st.moon.prepare(&st.gpu.queue, &frame.params);
        let input = || egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(size.0 as f32 / ppp, size.1 as f32 / ppp))),
            viewports: std::iter::once((
                egui::ViewportId::ROOT,
                egui::ViewportInfo { native_pixels_per_point: Some(ppp), ..Default::default() },
            ))
            .collect(),
            ..Default::default()
        };
        // Zwei Durchläufe: egui legt Schrift und Maße im ersten an.
        for _ in 0..2 {
            let mut enc = st.gpu.device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("render") });
            hud.paint(&st.gpu, &mut enc, off.view(), &st.moon, input(), Some(&model), &palette, size, ppp);
            off.submit_and_wait(&st.gpu, enc)?;
        }
        off.read(&st.gpu)?
    } else {
        off.render(&st.gpu, &st.moon, &frame.params)?;
        off.read(&st.gpu)?
    };
    write_png(path, size.0, size.1, &rgba, &receipt(session, settings, &c, &frame, size))?;
    println!("{}  {}×{}  {} UTC  EV {:.1}  ({})", path.display(), size.0, size.1, time::format_civil(c.jd), frame.ev, st.gpu.info.name);
    Ok(())
}

pub fn info(session: &Session, settings: &Settings) -> Result<()> {
    let c = Controls::new(settings);
    let sc = session.scene(settings.site, c.jd)?;
    println!("LUNA  {} UTC   Standort {:.4}° N {:.4}° O {:.0} m ({})", time::format_civil(sc.civil_jd), settings.site.lat_deg, settings.site.lon_deg, settings.site.height_m, settings.site_origin);
    println!("{}  ·  Erddrehung: {}", sc.phase_name(), sc.earth_rotation.label());
    for r in sc.readings() {
        println!("  {:<14} {:>12} {:<3} [{}]  {}  ·  {}", r.label, r.value_text(), r.unit, r.kind.letter(), r.source, r.bound);
    }
    println!("  weitere: Azimut {:.3}°  ·  Höhe mit Refraktion {:+.3}° [N]  ·  Sonnenhöhe {:+.2}°  ·  Colongitude {:.2}°  ·  Neigung der Achse zum Zenit {:+.2}°", sc.azimuth_deg, sc.altitude_refracted_deg, sc.sun_altitude_deg, sc.colongitude_deg(), sc.axis_tilt_from_zenith_deg());
    Ok(())
}

fn controls_at(jd: f64, view: View, mode: RenderMode) -> Controls {
    Controls { jd, rate: crate::session::RATE_REALTIME, paused: true, follows_clock: false, view, ev_manual: None, mode, show_help: false }
}

struct Outcome {
    lines: Vec<String>,
    failed: usize,
}

impl Outcome {
    fn check(&mut self, ok: bool, text: String) {
        if !ok {
            self.failed += 1;
        }
        self.lines.push(format!("  {}: {}", if ok { "besteht " } else { "VERFEHLT" }, text));
    }
}

/// Bildprüfungen: Kartenlage im Bild, Streumodell bei Vollmond, Schlagschatten, Wiederholbarkeit.
fn image_checks(session: &Session) -> Result<(Outcome, String)> {
    let st = stage(session)?;
    let mut out = Outcome { lines: Vec::new(), failed: 0 };
    let site = Site::new(51.4769, 0.0, 46.0)?;
    let size = (1600u32, 1600u32);

    // 1. Kartenlage im Bild: Der Shader sagt je Bildpunkt, welche Länge/Breite er zeigt.
    //    Unabhängig davon rechnet die Kamera aus, wo ein Ort im Bild liegen muss.
    let places: [(&str, f64, f64); 6] = [
        ("Apollo 11", 0.67416, 23.47314),
        ("Apollo 15", 26.13239, 3.63330),
        ("Apollo 17", 20.19110, 30.77168),
        ("Tycho", -43.31, -11.36),
        ("Copernicus", 9.62, -20.08),
        ("Grimaldi", -5.5, -68.3),
    ];
    let jd = time::parse_civil("2026-11-21T20:00:00").map_err(|e| anyhow!(e))?;
    for zoom in [1.0, 6.0] {
        for (name, lat, lon) in places {
            // Bei Zoom 6 den Ort in die Bildmitte holen: freie Drehung, bis er zur Kamera zeigt.
            let mut view = View { zoom, ..View::default() };
            if zoom > 1.0 {
                let sc = session.scene(site, jd)?;
                let cam0 = luna_render::Camera::from_scene(&sc, &view);
                let p = mul_mv(&sc.moon_me_to_j2000, from_lon_lat_deg(lon, lat));
                let (x, y, z) = (dot(p, cam0.right), dot(p, cam0.up), -dot(p, cam0.fwd));
                view.yaw = x.atan2(z);
                view.pitch = -(y / (x * x + z * z).sqrt()).atan();
            }
            let c = controls_at(jd, view, RenderMode::LonLat);
            let (rgba, frame) = render_rgba(&st, session, site, &c, size)?;
            let p_km = scale(mul_mv(&frame.scene.moon_me_to_j2000, from_lon_lat_deg(lon, lat)), MOON_RADIUS_KM);
            let Some((px, py)) = frame.camera.project(p_km, size.0, size.1) else {
                out.check(false, format!("{name} (Zoom {zoom}): liegt hinter der Kamera"));
                continue;
            };
            let visible = frame.camera.faces_camera(p_km) && px >= 1.0 && py >= 1.0 && px < size.0 as f64 - 1.0 && py < size.1 as f64 - 1.0;
            if !visible {
                out.check(false, format!("{name} (Zoom {zoom}): nicht im Bild"));
                continue;
            }
            let (ix, iy) = (px.floor() as usize, py.floor() as usize);
            let i = (iy * size.0 as usize + ix) * 4;
            let u = (rgba[i] as f64 * 256.0 + rgba[i + 1] as f64 + 0.5) / 65536.0;
            let v = (rgba[i + 2] as f64 * 256.0 + rgba[i + 3] as f64 + 0.5) / 65536.0;
            let (s_lon, s_lat) = ((u - 0.5) * 360.0, (0.5 - v) * 180.0);
            // Wo liegt der vom Shader gemeldete Ort laut Kamera? Er muss auf diesen Bildpunkt fallen.
            let q_km = scale(mul_mv(&frame.scene.moon_me_to_j2000, from_lon_lat_deg(s_lon, s_lat)), MOON_RADIUS_KM);
            let (qx, qy) = frame.camera.project(q_km, size.0, size.1).unwrap_or((f64::NAN, f64::NAN));
            let off_px = ((qx - (ix as f64 + 0.5)).powi(2) + (qy - (iy as f64 + 0.5)).powi(2)).sqrt();
            let off_km = angle(p_km, q_km) * MOON_RADIUS_KM;
            out.check(
                off_px <= 1.0,
                format!(
                    "{name} (Zoom {zoom}): erwartet bei Bildpunkt ({:.1}, {:.1}); der Shader zeigt dort {:.3}° N, {:.3}° O — Abstand {:.2} Bildpunkte, {:.2} km auf dem Mond",
                    px, py, s_lat, s_lon, off_px, off_km
                ),
            );
        }
    }

    // 2. Vollmond: kein dunkler Rand. Zeitpunkt mit kleinstem Phasenwinkel um den 24.11.2026 suchen.
    let base = time::parse_civil("2026-11-24T00:00:00").map_err(|e| anyhow!(e))?;
    let (mut best_jd, mut best_alpha) = (base, f64::MAX);
    for k in 0..(48 * 6) {
        let jd = base + k as f64 / 144.0;
        let a = session.scene(site, jd)?.phase_angle_deg;
        if a < best_alpha {
            (best_jd, best_alpha) = (jd, a);
        }
    }
    let c = controls_at(best_jd, View::default(), RenderMode::Gray);
    let (rgba, frame) = render_rgba(&st, session, site, &c, size)?;
    let (cx, cy) = frame.camera.project([0.0; 3], size.0, size.1).ok_or_else(|| anyhow!("Mondmitte nicht im Bild"))?;
    let r_px = frame.camera.disc_radius_px(size.0, size.1);
    let lin = |v: u8| -> f64 {
        let x = v as f64 / 255.0;
        if x <= 0.04045 { x / 12.92 } else { ((x + 0.055) / 1.055).powf(2.4) }
    };
    let sample = |fx: f64, fy: f64| -> f64 {
        let (mut s, mut n) = (0.0, 0.0);
        for dy in -2..=2i64 {
            for dx in -2..=2i64 {
                let (x, y) = ((fx as i64 + dx).clamp(0, size.0 as i64 - 1) as usize, (fy as i64 + dy).clamp(0, size.1 as i64 - 1) as usize);
                s += lin(rgba[(y * size.0 as usize + x) * 4]);
                n += 1.0;
            }
        }
        s / n
    };
    let centre = sample(cx, cy);
    let mut worst = f64::MAX;
    for k in 0..24 {
        let a = k as f64 * 15.0 * DEG;
        worst = worst.min(sample(cx + 0.9 * r_px * a.cos(), cy + 0.9 * r_px * a.sin()) / centre);
    }
    out.check(
        worst >= 0.80,
        format!(
            "Vollmond {} UTC (Phasenwinkel {:.2}°), graue Kugel: Helligkeit bei 90 % des Radius mindestens {:.0} % der Mitte (Grenze 80 %; ein Lambert-Modell läge bei 44 %)",
            time::format_civil(best_jd),
            best_alpha,
            worst * 100.0
        ),
    );

    // 3. Schlagschatten: Nahe der Lichtgrenze muss das volle Bild dunkler sein als ohne Schatten.
    let jd_q = time::parse_civil("2026-11-17T18:00:00").map_err(|e| anyhow!(e))?;
    let mean = |rgba: &[u8]| rgba.chunks(4).map(|p| lin(p[0])).sum::<f64>() / (rgba.len() / 4) as f64;
    let mut c_full = controls_at(jd_q, View::default(), RenderMode::Full);
    c_full.ev_manual = Some(12.0);
    let mut c_flat = c_full.clone();
    c_flat.mode = RenderMode::NoShadows;
    let (img_full, f_q) = render_rgba(&st, session, site, &c_full, size)?;
    let (img_flat, _) = render_rgba(&st, session, site, &c_flat, size)?;
    let (m_full, m_flat) = (mean(&img_full), mean(&img_flat));
    let differing = img_full.chunks(4).zip(img_flat.chunks(4)).filter(|(a, b)| a[0].abs_diff(b[0]) > 8).count();
    out.check(
        m_full < m_flat && differing > 2000,
        format!(
            "Schlagschatten am {} UTC (beleuchtet {:.0} %): mittlere Helligkeit {:.5} mit, {:.5} ohne Schatten; {} Bildpunkte deutlich verändert",
            time::format_civil(jd_q),
            f_q.scene.illuminated_fraction * 100.0,
            m_full,
            m_flat,
            differing
        ),
    );

    // 4. Wiederholbarkeit: dasselbe Kommando, dasselbe Bild.
    let (again, _) = render_rgba(&st, session, site, &c_full, size)?;
    let st2 = stage(session)?;
    let (fresh, _) = render_rgba(&st2, session, site, &c_full, size)?;
    out.check(again == img_full && fresh == img_full, "Wiederholbarkeit: dreimal dasselbe Bild erzeugt (einmal mit neu geöffneter Grafikkarte), Byte für Byte gleich".to_string());

    Ok((out, st.gpu.describe()))
}

pub fn verify(session: &Session) -> Result<bool> {
    let mut failed = 0usize;
    println!("LUNA — Selbstprüfung\n");
    println!("1. Daten");
    println!(
        "  besteht : {} Dateien, {:.0} MB, SHA-256 stimmt mit dem Manifest überein",
        session.manifest.raw.len() + session.manifest.forged.len(),
        session.checked_bytes as f64 / 1e6
    );
    let (a, b) = session.jd_range;
    println!("  Ephemeride DE440s nutzbar von {} bis {} UTC", time::format_civil(a), time::format_civil(b));
    let (ea, eb) = session.eph.earth_rot_domain();
    println!(
        "  Erddrehung gemessen/vorhergesagt von {} bis {} UTC; außerhalb gilt das Ausweichmodell (N)",
        time::format_civil(time::civil_jd(ea)),
        time::format_civil(time::civil_jd(eb))
    );

    println!("\n2. Rechenkette gegen eingefrorene Referenzwerte");
    let groups = verify::run(&session.eph, &session.paths.reference().join("horizons"))?;
    print!("{}", verify::format_report(&groups));
    for g in &groups {
        if !g.pass() {
            failed += 1;
        }
    }

    println!("\n3. Kartenlage (aus der Aufbereitung)");
    for c in &session.manifest.forge.checks {
        println!("  {c}");
        if c.starts_with("VERFEHLT") {
            failed += 1;
        }
    }
    // Versatz der beiden Mond-Koordinatensysteme.
    let pa_to_me = session.eph.pa_to_me();
    let shift = angle([1.0, 0.0, 0.0], mul_mv(&pa_to_me, [1.0, 0.0, 0.0])) * MOON_RADIUS_KM * 1000.0;
    println!("  Hinweis: Hauptachsensystem → Kartensystem verschiebt den Nullpunkt um {:.0} m; die Korrektur ist immer angewendet", shift);

    println!("\n4. Bild");
    let (img, gpu) = image_checks(session)?;
    println!("  Grafikkarte: {gpu}");
    for l in &img.lines {
        println!("{l}");
    }
    failed += img.failed;

    println!();
    if failed == 0 {
        println!("ERGEBNIS: alle Prüfungen bestanden.");
    } else {
        println!("ERGEBNIS: {failed} Prüfung(en) verfehlt.");
    }
    println!("Nicht automatisch geprüft: Vergleich mit eigenen Fotos, zweiter Rechenweg mit Skyfield, Schriftschärfe und Leerlauflast am echten Fenster.");
    Ok(failed == 0)
}

fn stats(ms: &mut [f64]) -> serde_json::Value {
    ms.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let n = ms.len();
    serde_json::json!({
        "bilder": n,
        "mittel_ms": (ms.iter().sum::<f64>() / n as f64 * 1000.0).round() / 1000.0,
        "median_ms": ms[n / 2],
        "p95_ms": ms[(n as f64 * 0.95) as usize],
        "max_ms": ms[n - 1],
    })
}

/// Feste Kamerafahrt bei 2560×1440: Zeitraffer, Drehung, Zoom. Misst die reine Bildzeit
/// der Grafikkarte (Zeichnen bis fertig), ohne Fenster und ohne Warten auf den Bildschirm.
pub fn bench(session: &Session, settings: &Settings) -> Result<()> {
    let st = stage(session)?;
    let size = if settings.size == (1600, 1600) { (2560, 1440) } else { settings.size };
    let off = Offscreen::new(&st.gpu, size.0, size.1);
    let jd0 = time::parse_civil("2026-11-17T18:00:00").map_err(|e| anyhow!(e))?;
    let mut result = serde_json::Map::new();
    for (name, zoom) in [("vollscheibe_zoom_1", 1.0), ("ausschnitt_zoom_4", 4.0), ("bildfuellend_zoom_12", 12.0)] {
        let mut gpu_ms = Vec::new();
        let mut cpu_ms = Vec::new();
        for i in 0..260 {
            let t = i as f64;
            let view = View { zoom, yaw: (t * 0.7).to_radians().sin() * 0.6, pitch: (t * 0.4).to_radians().sin() * 0.3, ..View::default() };
            let c = controls_at(jd0 + t / 24.0, view, RenderMode::Full);
            let t0 = Instant::now();
            let frame = Frame::build(session, settings.site, &c, size.0, size.1)?;
            let t1 = Instant::now();
            off.render(&st.gpu, &st.moon, &frame.params)?;
            if i >= 20 {
                cpu_ms.push((t1 - t0).as_secs_f64() * 1000.0);
                gpu_ms.push(t1.elapsed().as_secs_f64() * 1000.0);
            }
        }
        result.insert(name.into(), serde_json::json!({ "bild": stats(&mut gpu_ms), "rechnung": stats(&mut cpu_ms) }));
    }
    let budget = 16.7;
    let worst = result.values().map(|v| v["bild"]["p95_ms"].as_f64().unwrap_or(f64::MAX) + v["rechnung"]["p95_ms"].as_f64().unwrap_or(f64::MAX)).fold(0.0, f64::max);
    let report = serde_json::json!({
        "grafikkarte": st.gpu.describe(),
        "bildgroesse": format!("{}x{}", size.0, size.1),
        "kartenspeicher_mb": (st.moon.texture_bytes as f64 / 1e6).round(),
        "budget_ms": budget,
        "schlechtester_p95_ms": (worst * 1000.0).round() / 1000.0,
        "im_budget": worst <= budget,
        "szenen": result,
        "hinweis": "Gemessen ohne Fenster: Zeichnen bis die Grafikkarte fertig ist, plus Rechnung der Szene. Die Darstellung im Fenster (Compositor, Instrumentenschicht) ist darin nicht enthalten.",
    });
    println!("{}", serde_json::to_string_pretty(&report)?);
    if worst > budget {
        bail!("Bildzeit über dem Budget von {budget} ms");
    }
    Ok(())
}
