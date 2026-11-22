//! LUNA Instrumentenschicht ("Okularplatte").
//!
//! Zeichnet ausschließlich Linien, Skalen und Zahlen über das Mondbild. egui dient
//! nur als Gerüst für Text und Eingabe; kein Standard-Widget ist sichtbar.
//! Die Schicht kennt nur das fertige `HudModel` und rechnet selbst nichts aus.

use egui::{Align2, Color32, FontFamily, FontId, Pos2, Rect, Stroke, StrokeKind, Vec2};
use luna_core::Reading;

/// Farben der Instrumentenschicht. Das Mondbild selbst wird nie eingefärbt.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Palette {
    pub accent: Color32,
    pub foreground: Color32,
    pub warning: Color32,
    /// Woher die Farben stammen, für die Anzeige.
    pub from_theme: bool,
}

impl Default for Palette {
    fn default() -> Self {
        Self {
            accent: Color32::from_rgb(0xd8, 0xd2, 0xc4),
            foreground: Color32::from_rgb(0x8f, 0x8b, 0x82),
            warning: Color32::from_rgb(0xe1, 0xba, 0x78),
            from_theme: false,
        }
    }
}

fn parse_hex(v: &str) -> Option<Color32> {
    let v = v.trim().trim_matches('"').trim_matches('\'');
    let h = v.strip_prefix('#')?;
    if h.len() != 6 {
        return None;
    }
    let n = u32::from_str_radix(h, 16).ok()?;
    Some(Color32::from_rgb((n >> 16) as u8, (n >> 8) as u8, n as u8))
}

impl Palette {
    /// Liest Akzent- und Schriftfarbe aus der laufenden Omarchy-Palette (`colors.toml`).
    /// Fehlt die Datei oder ein Wert, bleibt die neutrale Vorgabe.
    pub fn from_colors_toml(text: &str) -> Self {
        let mut p = Palette::default();
        let get = |key: &str| -> Option<Color32> {
            text.lines().find_map(|l| {
                let (k, v) = l.split_once('=')?;
                (k.trim() == key).then(|| parse_hex(v)).flatten()
            })
        };
        if let Some(c) = get("accent").or_else(|| get("color4")) {
            p.accent = c;
            p.from_theme = true;
        }
        if let Some(c) = get("foreground").or_else(|| get("color7")) {
            p.foreground = c;
            p.from_theme = true;
        }
        if let Some(c) = get("color3") {
            p.warning = c;
        }
        p
    }

    fn dim(&self) -> Color32 {
        self.foreground.gamma_multiply(0.55)
    }
    fn hair(&self) -> Color32 {
        self.foreground.gamma_multiply(0.35)
    }
}

/// Eine Zeile zur Herkunft des Bildes.
#[derive(Clone, Debug)]
pub struct ImageRow {
    pub label: &'static str,
    pub value: String,
    pub kind: &'static str,
    pub detail: &'static str,
}

/// Lage der Mondscheibe im Fenster (in Punkten), für die Marken am Rand.
#[derive(Clone, Copy, Debug)]
pub struct DiscOverlay {
    pub center: [f32; 2],
    pub radius: f32,
    /// Richtung des Mondnordpols im Bild (Einheitsvektor, y nach unten), falls bestimmbar.
    pub lunar_north: Option<[f32; 2]>,
    /// Richtung "oben am Himmel" (Zenit) im Bild.
    pub zenith: Option<[f32; 2]>,
}

#[derive(Clone, Debug, Default)]
pub struct HudModel {
    pub time_text: String,
    pub rate_text: String,
    pub site_text: String,
    pub phase_text: String,
    pub view_text: String,
    pub readings: Vec<Reading>,
    pub image_rows: Vec<ImageRow>,
    pub notices: Vec<String>,
    pub status_text: String,
    pub show_help: bool,
    pub disc: Option<DiscOverlay>,
}

/// Dichtestufe nach verfügbarer Fläche (Regel "Adaptive Instrument Density").
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Density {
    Full,
    Compact,
    Status,
    Beacon,
}

pub fn density_for(size: Vec2) -> Density {
    let m = size.x.min(size.y);
    if m < 200.0 {
        Density::Beacon
    } else if m < 380.0 || size.x < 460.0 {
        Density::Status
    } else if size.x < 900.0 || size.y < 560.0 {
        Density::Compact
    } else {
        Density::Full
    }
}

pub const HELP_LINES: &[(&str, &str)] = &[
    ("Mausrad", "Zeit ziehen, 20 min je Raste"),
    ("Umschalt + Mausrad", "Zeit ziehen, 1 Tag je Raste"),
    ("Strg + Mausrad, + / -", "Zoom"),
    ("Ziehen", "Kugel drehen (freie Ansicht)"),
    ("Doppelklick, Pos1", "zurück zum Blick vom Standort"),
    ("Pfeil rechts / links", "Zeitraffer schneller / langsamer, rückwärts"),
    ("Leertaste", "Zeit anhalten / weiterlaufen"),
    ("N", "Jetzt, Echtzeit"),
    ("E / Umschalt + E", "Belichtung heller / dunkler (1/3 Stufe)"),
    ("A", "Belichtungsautomatik"),
    ("O", "Ausrichtung: Zenit, Himmelsnord, Mondnord"),
    ("R", "Bild: voll, ohne Schlagschatten, glatte Kugel"),
    ("S", "Bild als PNG-Beleg speichern"),
    ("H, F1", "diese Hilfe"),
    ("Q", "beenden"),
];

fn mono(size: f32) -> FontId {
    FontId::new(size, FontFamily::Monospace)
}

fn chip(painter: &egui::Painter, right_center: Pos2, kind: &str, color: Color32) {
    let size = 15.0;
    let rect = Rect::from_min_size(Pos2::new(right_center.x - size, right_center.y - size / 2.0), Vec2::splat(size));
    painter.rect_stroke(rect, 0.0, Stroke::new(1.0, color), StrokeKind::Inside);
    painter.text(rect.center(), Align2::CENTER_CENTER, kind, mono(10.5), color);
}

/// Zeichnet die Instrumentenschicht. Gibt die Dichtestufe zurück.
pub fn draw(ctx: &egui::Context, model: &HudModel, pal: &Palette) -> Density {
    let rect = ctx.content_rect();
    let density = density_for(rect.size());
    let painter = ctx.layer_painter(egui::LayerId::new(egui::Order::Foreground, egui::Id::new("luna-hud")));
    let pointer = ctx.pointer_hover_pos();
    let margin = if density == Density::Full { 18.0 } else { 10.0 };
    let label_font = mono(11.0);
    let value_font = mono(14.0);

    if density == Density::Beacon {
        // Kleinste Stufe: nur der Beleuchtungsgrad.
        if let Some(r) = model.readings.first() {
            painter.text(
                rect.left_bottom() + Vec2::new(6.0, -5.0),
                Align2::LEFT_BOTTOM,
                format!("{} {}", r.value_text(), r.unit),
                mono(12.0),
                pal.accent,
            );
        }
        return density;
    }

    // --- Marken am Scheibenrand ---
    if let (Some(disc), true) = (model.disc, density != Density::Status) {
        let c = Pos2::new(disc.center[0], disc.center[1]);
        let tick = |dir: Option<[f32; 2]>, label: &str, color: Color32| {
            if let Some(d) = dir {
                let v = Vec2::new(d[0], d[1]);
                let a = c + v * (disc.radius + 7.0);
                let b = c + v * (disc.radius + 19.0);
                if rect.shrink(4.0).contains(b) {
                    painter.line_segment([a, b], Stroke::new(1.0, color));
                    painter.text(c + v * (disc.radius + 30.0), Align2::CENTER_CENTER, label, mono(10.0), color);
                }
            }
        };
        tick(disc.lunar_north, "N", pal.accent);
        tick(disc.zenith, "Z", pal.dim());
    }

    // --- Kopf links oben ---
    let mut y = rect.top() + margin;
    let x = rect.left() + margin;
    let line = |text: &str, font: FontId, color: Color32, y: &mut f32| {
        let r = painter.text(Pos2::new(x, *y), Align2::LEFT_TOP, text, font, color);
        *y += r.height() + 3.0;
    };
    if density == Density::Full {
        line("LUNA  ·  ERSTES LICHT", label_font.clone(), pal.dim(), &mut y);
        y += 2.0;
    }
    line(&model.time_text, value_font.clone(), pal.accent, &mut y);
    if density != Density::Status {
        line(&model.rate_text, label_font.clone(), pal.foreground, &mut y);
        line(&model.site_text, label_font.clone(), pal.foreground, &mut y);
        line(&model.phase_text, label_font.clone(), pal.foreground, &mut y);
        line(&model.view_text, label_font.clone(), pal.dim(), &mut y);
    }
    y += 4.0;
    for n in &model.notices {
        line(n, label_font.clone(), pal.warning, &mut y);
    }

    if density == Density::Status {
        if let Some(r) = model.readings.first() {
            painter.text(
                rect.left_bottom() + Vec2::new(margin, -margin),
                Align2::LEFT_BOTTOM,
                format!("{}  {} {}", r.label, r.value_text(), r.unit),
                value_font,
                pal.accent,
            );
        }
        return density;
    }

    // --- Messwertspalte rechts ---
    let col_w = 292.0;
    let right = rect.right() - margin;
    let left = right - col_w;
    let row_h = 21.0;
    let mut y = rect.top() + margin;
    let mut detail: Option<String> = None;

    painter.text(Pos2::new(left, y), Align2::LEFT_TOP, "MESSWERTE", label_font.clone(), pal.dim());
    y += 16.0;
    painter.line_segment([Pos2::new(left, y), Pos2::new(right, y)], Stroke::new(1.0, pal.hair()));
    y += 5.0;

    let shown = if density == Density::Full { model.readings.len() } else { model.readings.len().min(4) };
    for r in model.readings.iter().take(shown) {
        let row = Rect::from_min_size(Pos2::new(left, y), Vec2::new(col_w, row_h));
        let mid = row.center().y;
        painter.text(Pos2::new(left, mid), Align2::LEFT_CENTER, r.label, label_font.clone(), pal.foreground);
        // Zahlen rechtsbündig an fester Spalte, Einheit dahinter.
        painter.text(Pos2::new(right - 62.0, mid), Align2::RIGHT_CENTER, r.value_text(), value_font.clone(), pal.accent);
        painter.text(Pos2::new(right - 56.0, mid), Align2::LEFT_CENTER, r.unit, label_font.clone(), pal.foreground);
        chip(&painter, Pos2::new(right, mid), r.kind.letter(), pal.foreground);
        if pointer.is_some_and(|p| row.contains(p)) {
            detail = Some(format!("{} ({})  ·  {}  ·  {}", r.kind.letter(), r.kind.word(), r.source, r.bound));
        }
        y += row_h;
    }

    if density == Density::Full {
        y += 12.0;
        painter.text(Pos2::new(left, y), Align2::LEFT_TOP, "BILD", label_font.clone(), pal.dim());
        y += 16.0;
        painter.line_segment([Pos2::new(left, y), Pos2::new(right, y)], Stroke::new(1.0, pal.hair()));
        y += 5.0;
        for r in &model.image_rows {
            let row = Rect::from_min_size(Pos2::new(left, y), Vec2::new(col_w, row_h));
            let mid = row.center().y;
            painter.text(Pos2::new(left, mid), Align2::LEFT_CENTER, r.label, label_font.clone(), pal.foreground);
            painter.text(Pos2::new(right - 24.0, mid), Align2::RIGHT_CENTER, &r.value, mono(12.0), pal.accent);
            chip(&painter, Pos2::new(right, mid), r.kind, pal.foreground);
            if pointer.is_some_and(|p| row.contains(p)) {
                detail = Some(r.detail.to_string());
            }
            y += row_h;
        }
        y += 10.0;
        painter.text(
            Pos2::new(left, y),
            Align2::LEFT_TOP,
            "M gemessen   B berechnet\nN Näherung   K künstlerisch",
            mono(10.5),
            pal.dim(),
        );
    }

    // --- Fußzeile ---
    let foot_y = rect.bottom() - margin;
    if let Some(d) = detail {
        painter.text(Pos2::new(x, foot_y), Align2::LEFT_BOTTOM, d, label_font.clone(), pal.accent);
    } else if density == Density::Full {
        painter.text(Pos2::new(x, foot_y), Align2::LEFT_BOTTOM, &model.status_text, label_font.clone(), pal.dim());
        painter.text(Pos2::new(right, foot_y), Align2::RIGHT_BOTTOM, "H  HILFE", label_font.clone(), pal.dim());
    }

    // --- Hilfe ---
    if model.show_help {
        let w = 560.0f32.min(rect.width() - 2.0 * margin);
        let h = HELP_LINES.len() as f32 * 19.0 + 44.0;
        let panel = Rect::from_center_size(rect.center(), Vec2::new(w, h));
        painter.rect_filled(panel, 0.0, Color32::from_black_alpha(225));
        painter.rect_stroke(panel, 0.0, Stroke::new(1.0, pal.hair()), StrokeKind::Inside);
        let mut y = panel.top() + 14.0;
        painter.text(Pos2::new(panel.left() + 16.0, y), Align2::LEFT_TOP, "BEDIENUNG", label_font.clone(), pal.dim());
        y += 22.0;
        for (key, what) in HELP_LINES {
            painter.text(Pos2::new(panel.left() + 16.0, y), Align2::LEFT_TOP, *key, mono(12.0), pal.accent);
            painter.text(Pos2::new(panel.left() + 220.0, y), Align2::LEFT_TOP, *what, mono(12.0), pal.foreground);
            y += 19.0;
        }
    }
    density
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn palette_reads_omarchy_colors() {
        let p = Palette::from_colors_toml("# x\naccent = \"#11ff25\"\nforeground = \"#507350\"\ncolor3 = \"#90d762\"\n");
        assert_eq!(p.accent, Color32::from_rgb(0x11, 0xff, 0x25));
        assert_eq!(p.foreground, Color32::from_rgb(0x50, 0x73, 0x50));
        assert!(p.from_theme);
        let q = Palette::from_colors_toml("accent = \"bad\"\n");
        assert_eq!(q, Palette::default());
    }

    #[test]
    fn density_levels() {
        assert_eq!(density_for(Vec2::new(1600.0, 1000.0)), Density::Full);
        assert_eq!(density_for(Vec2::new(700.0, 600.0)), Density::Compact);
        assert_eq!(density_for(Vec2::new(400.0, 300.0)), Density::Status);
        assert_eq!(density_for(Vec2::new(180.0, 120.0)), Density::Beacon);
    }
}
