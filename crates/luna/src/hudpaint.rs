//! Bringt die Instrumentenschicht auf die Grafikkarte. Fenster und fensterlose
//! Ausgabe nutzen denselben Weg, damit ein Bildtest dasselbe zeigt wie der Bildschirm.

use std::sync::Arc;

use luna_render::{Gpu, MoonRenderer};
use luna_ui::{HudModel, Palette};

pub struct HudPainter {
    pub ctx: egui::Context,
    renderer: egui_wgpu::Renderer,
}

/// Sucht die Monospace-Schrift des Systems über fontconfig. Fehlt sie, bleibt die eingebaute.
fn system_mono_font() -> Option<Vec<u8>> {
    let out = std::process::Command::new("fc-match").args(["-f", "%{file}", "monospace"]).output().ok()?;
    if !out.status.success() {
        return None;
    }
    let path = String::from_utf8(out.stdout).ok()?;
    let path = path.trim();
    if !(path.ends_with(".ttf") || path.ends_with(".otf")) {
        return None;
    }
    std::fs::read(path).ok()
}

/// Liest die laufende Omarchy-Palette; `None`, wenn es keine gibt.
pub fn theme_path() -> Option<std::path::PathBuf> {
    let home = std::env::var_os("HOME").map(std::path::PathBuf::from)?;
    let state = std::env::var_os("XDG_STATE_HOME").map(std::path::PathBuf::from).unwrap_or_else(|| home.join(".local/state"));
    let config = std::env::var_os("XDG_CONFIG_HOME").map(std::path::PathBuf::from).unwrap_or_else(|| home.join(".config"));
    [state, config].into_iter().map(|b| b.join("omarchy/current/theme/colors.toml")).find(|p| p.is_file())
}

pub fn load_palette() -> Palette {
    theme_path()
        .and_then(|p| std::fs::read_to_string(p).ok())
        .map(|t| Palette::from_colors_toml(&t))
        .unwrap_or_default()
}

impl HudPainter {
    pub fn new(gpu: &Gpu, format: wgpu::TextureFormat) -> Self {
        let ctx = egui::Context::default();
        let mut fonts = egui::FontDefinitions::default();
        if let Some(bytes) = system_mono_font() {
            fonts.font_data.insert("system-mono".to_owned(), Arc::new(egui::FontData::from_owned(bytes)));
            for family in [egui::FontFamily::Monospace, egui::FontFamily::Proportional] {
                fonts.families.entry(family).or_default().insert(0, "system-mono".to_owned());
            }
        }
        ctx.set_fonts(fonts);
        let renderer = egui_wgpu::Renderer::new(&gpu.device, format, egui_wgpu::RendererOptions::default());
        Self { ctx, renderer }
    }

    /// Zeichnet Mond und Instrumentenschicht in `target`. Gibt die egui-Ausgabe für das Fenster zurück.
    #[allow(clippy::too_many_arguments)]
    pub fn paint(
        &mut self,
        gpu: &Gpu,
        encoder: &mut wgpu::CommandEncoder,
        target: &wgpu::TextureView,
        moon: &MoonRenderer,
        raw_input: egui::RawInput,
        model: Option<&HudModel>,
        palette: &Palette,
        size: (u32, u32),
        pixels_per_point: f32,
    ) -> egui::PlatformOutput {
        let output = self.ctx.run_ui(raw_input, |ui| {
            if let Some(m) = model {
                luna_ui::draw(ui.ctx(), m, palette);
            }
        });
        let jobs = self.ctx.tessellate(output.shapes, pixels_per_point);
        let screen = egui_wgpu::ScreenDescriptor { size_in_pixels: [size.0, size.1], pixels_per_point };
        for (id, delta) in &output.textures_delta.set {
            self.renderer.update_texture(&gpu.device, &gpu.queue, *id, delta);
        }
        self.renderer.update_buffers(&gpu.device, &gpu.queue, encoder, &jobs, &screen);
        {
            let pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("bild"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: target,
                    resolve_target: None,
                    depth_slice: None,
                    ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::BLACK), store: wgpu::StoreOp::Store },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            let mut pass = pass.forget_lifetime();
            moon.draw(&mut pass);
            self.renderer.render(&mut pass, &jobs, &screen);
        }
        for id in &output.textures_delta.free {
            self.renderer.free_texture(id);
        }
        output.platform_output
    }
}
