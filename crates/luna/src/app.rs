//! Das Fenster: Eingaben, Zeitlauf, Zeichnen nur bei Bedarf.

use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};

use anyhow::{anyhow, Result};
use luna_render::{Gpu, MoonRenderer, Offscreen, OFFSCREEN_FORMAT};
use luna_ui::Palette;
use winit::application::ApplicationHandler;
use winit::dpi::{LogicalSize, PhysicalPosition};
use winit::event::{ElementState, MouseButton, MouseScrollDelta, StartCause, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::keyboard::{Key, ModifiersState, NamedKey};
use winit::platform::wayland::WindowAttributesExtWayland;
use winit::window::{Window, WindowId};

use crate::headless;
use crate::hudpaint::{load_palette, theme_path, HudPainter};
use crate::session::{hud_model, now_jd, Controls, Frame, Session, Settings, RATES, RATE_REALTIME, ZOOM_MAX, ZOOM_MIN};

/// Schrittweite des Mausrads beim Ziehen der Zeit, in Tagen je Raste.
const WHEEL_STEP_DAYS: f64 = 20.0 / 1440.0;
const WHEEL_STEP_DAYS_SHIFT: f64 = 1.0;

struct Running {
    window: Arc<Window>,
    surface: wgpu::Surface<'static>,
    config: wgpu::SurfaceConfiguration,
    gpu: Gpu,
    moon: MoonRenderer,
    hud: HudPainter,
    egui_state: egui_winit::State,
}

struct App {
    session: Session,
    settings: Settings,
    controls: Controls,
    running: Option<Running>,
    error: Option<anyhow::Error>,

    palette: Palette,
    palette_stamp: Option<SystemTime>,
    palette_checked: Instant,

    modifiers: ModifiersState,
    cursor: PhysicalPosition<f64>,
    dragging: bool,
    drag_moved: bool,
    last_click: Option<Instant>,

    /// Zeitpunkt, zu dem `controls.jd` zuletzt fortgeschrieben wurde.
    last_tick: Instant,
    /// Zeit bis zu der nach einer Eingabe noch flüssig gezeichnet wird.
    active_until: Instant,
    frame_ms: f64,
    frames: u64,
    notice: Option<(String, Instant)>,
}

impl App {
    fn create(&mut self, el: &ActiveEventLoop) -> Result<()> {
        let attrs = Window::default_attributes()
            .with_title("LUNA")
            .with_name("luna", "luna")
            .with_inner_size(LogicalSize::new(1280.0, 800.0));
        let window = Arc::new(el.create_window(attrs).map_err(|e| anyhow!("Fenster ließ sich nicht öffnen: {e}"))?);

        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
            backends: wgpu::Backends::VULKAN,
            ..wgpu::InstanceDescriptor::new_with_display_handle(Box::new(el.owned_display_handle()))
        });
        let surface = instance.create_surface(window.clone()).map_err(|e| anyhow!("Zeichenfläche: {e}"))?;
        let gpu = pollster::block_on(Gpu::new(instance, Some(&surface)))?;
        eprintln!("LUNA: Grafikkarte {}", gpu.describe());

        let caps = surface.get_capabilities(&gpu.adapter);
        let format = caps
            .formats
            .iter()
            .copied()
            .find(|f| matches!(f, wgpu::TextureFormat::Bgra8Unorm | wgpu::TextureFormat::Rgba8Unorm))
            .ok_or_else(|| anyhow!("Die Zeichenfläche bietet kein 8-Bit-Format ohne eigene sRGB-Kodierung an: {:?}", caps.formats))?;
        let size = window.inner_size();
        let mut config = surface
            .get_default_config(&gpu.adapter, size.width.max(1), size.height.max(1))
            .ok_or_else(|| anyhow!("Die Grafikkarte kann nicht in dieses Fenster zeichnen"))?;
        config.format = format;
        config.present_mode = wgpu::PresentMode::AutoVsync;
        config.alpha_mode = wgpu::CompositeAlphaMode::Opaque;
        if !caps.alpha_modes.contains(&config.alpha_mode) {
            config.alpha_mode = caps.alpha_modes[0];
        }
        surface.configure(&gpu.device, &config);

        let moon = MoonRenderer::new(&gpu, &self.session.paths, &self.session.manifest, format)?;
        let hud = HudPainter::new(&gpu, format);
        let egui_state = egui_winit::State::new(
            hud.ctx.clone(),
            egui::ViewportId::ROOT,
            &*window,
            Some(window.scale_factor() as f32),
            None,
            Some(gpu.device.limits().max_texture_dimension_2d as usize),
        );
        self.running = Some(Running { window, surface, config, gpu, moon, hud, egui_state });
        Ok(())
    }

    fn fail(&mut self, el: &ActiveEventLoop, e: anyhow::Error) {
        self.error = Some(e);
        el.exit();
    }

    fn touch(&mut self) {
        self.active_until = Instant::now() + Duration::from_millis(250);
        if let Some(r) = &self.running {
            r.window.request_redraw();
        }
    }

    /// Schreibt die simulierte Zeit fort.
    fn advance_time(&mut self) {
        let now = Instant::now();
        let dt = (now - self.last_tick).as_secs_f64();
        self.last_tick = now;
        let c = &mut self.controls;
        if c.paused {
            return;
        }
        if c.rate == RATE_REALTIME && c.follows_clock {
            c.jd = now_jd();
        } else {
            // Im Zeitraffer große Sprünge nach einem Stocken des Fensters vermeiden;
            // in Echtzeit zählt die wirklich vergangene Zeit.
            let dt = if c.rate == RATE_REALTIME { dt } else { dt.min(0.1) };
            c.jd += RATES[c.rate].0 * dt / 86400.0;
        }
        let clamped = self.session.clamp_jd(c.jd);
        if clamped != c.jd {
            c.jd = clamped;
            c.paused = true;
            self.notice = Some(("ENDE DER EPHEMERIDE DE440s ERREICHT · Zeit angehalten".into(), Instant::now() + Duration::from_secs(6)));
        }
    }

    fn set_time(&mut self, jd: f64) {
        self.controls.jd = self.session.clamp_jd(jd);
        self.controls.follows_clock = false;
        self.touch();
    }

    fn reload_palette(&mut self) {
        if self.palette_checked.elapsed() < Duration::from_secs(2) {
            return;
        }
        self.palette_checked = Instant::now();
        let stamp = theme_path().and_then(|p| std::fs::metadata(p).ok()).and_then(|m| m.modified().ok());
        if stamp != self.palette_stamp {
            self.palette_stamp = stamp;
            self.palette = load_palette();
        }
    }

    fn save_receipt(&mut self) -> Result<String> {
        let r = self.running.as_ref().ok_or_else(|| anyhow!("kein Fenster"))?;
        let size = (r.config.width, r.config.height);
        let frame = Frame::build(&self.session, self.settings.site, &self.controls, size.0, size.1)?;
        // Eigener Renderer im Format der fensterlosen Ausgabe, damit das Bild dem von `--render` gleicht.
        let moon = MoonRenderer::new(&r.gpu, &self.session.paths, &self.session.manifest, OFFSCREEN_FORMAT)?;
        let off = Offscreen::new(&r.gpu, size.0, size.1);
        off.render(&r.gpu, &moon, &frame.params)?;
        let rgba = off.read(&r.gpu)?;
        let home = std::env::var_os("HOME").map(std::path::PathBuf::from).ok_or_else(|| anyhow!("HOME ist nicht gesetzt"))?;
        let dir = home.join("Pictures/LUNA");
        std::fs::create_dir_all(&dir)?;
        let stamp = luna_core::time::format_civil(self.controls.jd).replace([':', '-'], "").replace(' ', "-");
        let path = dir.join(format!("luna-{stamp}Z.png"));
        headless::write_png(&path, size.0, size.1, &rgba, &headless::receipt(&self.session, &self.settings, &self.controls, &frame, size))?;
        Ok(path.display().to_string())
    }

    fn redraw(&mut self) -> Result<()> {
        self.advance_time();
        self.reload_palette();
        let t0 = Instant::now();
        let Some(r) = self.running.as_mut() else { return Ok(()) };
        let (w, h) = (r.config.width, r.config.height);
        if w == 0 || h == 0 {
            return Ok(());
        }
        let frame = Frame::build(&self.session, self.settings.site, &self.controls, w, h)?;
        r.moon.prepare(&r.gpu.queue, &frame.params);

        let ppp = r.window.scale_factor() as f32;
        let mut notices = Vec::new();
        if let Some((text, until)) = &self.notice {
            if Instant::now() < *until {
                notices.push(text.clone());
            } else {
                self.notice = None;
            }
        }
        let status = format!(
            "{}  ·  BILD {:.1} ms  ·  KARTEN {:.0} MB  ·  FARBEN {}",
            r.gpu.info.name,
            self.frame_ms,
            r.moon.texture_bytes as f64 / 1e6,
            if self.palette.from_theme { "OMARCHY-THEME" } else { "VORGABE" }
        );
        let model = hud_model(&self.session, &self.settings, &self.controls, &frame, w, h, ppp, status, &notices);

        let output = match r.surface.get_current_texture() {
            // "Suboptimal": Das Bild ist verwendbar; die Fläche wird beim nächsten Größenwechsel neu eingerichtet.
            wgpu::CurrentSurfaceTexture::Success(t) | wgpu::CurrentSurfaceTexture::Suboptimal(t) => t,
            wgpu::CurrentSurfaceTexture::Outdated | wgpu::CurrentSurfaceTexture::Lost => {
                r.surface.configure(&r.gpu.device, &r.config);
                r.window.request_redraw();
                return Ok(());
            }
            // Verdeckt oder gerade kein Bild frei: beim nächsten Anlass erneut versuchen.
            _ => return Ok(()),
        };
        let view = output.texture.create_view(&wgpu::TextureViewDescriptor::default());
        let mut encoder = r.gpu.device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("bild") });
        let raw_input = r.egui_state.take_egui_input(&r.window);
        let platform = r.hud.paint(&r.gpu, &mut encoder, &view, &r.moon, raw_input, Some(&model), &self.palette, (w, h), ppp);
        r.egui_state.handle_platform_output(&r.window, platform);
        r.gpu.queue.submit([encoder.finish()]);
        let ms = t0.elapsed().as_secs_f64() * 1000.0;
        self.frame_ms = if self.frames == 0 { ms } else { self.frame_ms * 0.9 + ms * 0.1 };
        self.frames += 1;
        r.window.pre_present_notify();
        output.present();
        Ok(())
    }

    fn key(&mut self, el: &ActiveEventLoop, key: &Key) {
        let shift = self.modifiers.shift_key();
        let c = &mut self.controls;
        match key {
            Key::Named(NamedKey::Escape) => c.show_help = false,
            Key::Named(NamedKey::F1) => c.show_help = !c.show_help,
            Key::Named(NamedKey::Space) => {
                // Nach dem Anhalten läuft die Zeit ab der eingestellten Stelle weiter.
                c.paused = !c.paused;
            }
            Key::Named(NamedKey::ArrowRight) => {
                c.rate = (c.rate + 1).min(RATES.len() - 1);
                c.paused = false;
                c.follows_clock = false;
            }
            Key::Named(NamedKey::ArrowLeft) => {
                c.rate = c.rate.saturating_sub(1);
                c.paused = false;
                c.follows_clock = false;
            }
            Key::Named(NamedKey::Home) => {
                c.view.yaw = 0.0;
                c.view.pitch = 0.0;
                c.view.zoom = 1.0;
            }
            Key::Character(s) => match s.to_lowercase().as_str() {
                "q" => el.exit(),
                "h" => c.show_help = !c.show_help,
                "n" => {
                    c.jd = now_jd();
                    c.rate = RATE_REALTIME;
                    c.paused = false;
                    c.follows_clock = true;
                }
                "a" => c.ev_manual = None,
                "e" => {
                    let frame_ev = Frame::build(&self.session, self.settings.site, c, 64, 64).map(|f| f.ev).unwrap_or(12.0);
                    let cur = c.ev_manual.unwrap_or((frame_ev * 3.0).round() / 3.0);
                    // Kleinerer Lichtwert = längere Belichtung = helleres Bild.
                    c.ev_manual = Some((cur + if shift { 1.0 / 3.0 } else { -1.0 / 3.0 }).clamp(-6.0, 20.0));
                }
                "o" => c.view.up = c.view.up.next(),
                "r" => c.mode = c.mode.next(),
                "+" | "=" => c.view.zoom = (c.view.zoom * 1.15).min(ZOOM_MAX),
                "-" => c.view.zoom = (c.view.zoom / 1.15).max(ZOOM_MIN),
                "s" => {
                    let text = match self.save_receipt() {
                        Ok(p) => format!("BELEG GESPEICHERT · {p}"),
                        Err(e) => format!("BELEG NICHT GESPEICHERT · {e:#}"),
                    };
                    self.notice = Some((text, Instant::now() + Duration::from_secs(6)));
                }
                _ => {}
            },
            _ => {}
        }
        self.last_tick = Instant::now();
        self.touch();
    }

    fn wants_continuous(&self) -> bool {
        self.controls.animating() || self.dragging || Instant::now() < self.active_until
    }
}

impl ApplicationHandler for App {
    fn resumed(&mut self, el: &ActiveEventLoop) {
        if self.running.is_none() {
            if let Err(e) = self.create(el) {
                self.fail(el, e);
            }
        }
    }

    fn new_events(&mut self, _el: &ActiveEventLoop, cause: StartCause) {
        if let StartCause::ResumeTimeReached { .. } = cause {
            if let Some(r) = &self.running {
                r.window.request_redraw();
            }
        }
    }

    fn window_event(&mut self, el: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        let Some(r) = self.running.as_mut() else { return };
        let response = r.egui_state.on_window_event(&r.window, &event);
        // egui-winit meldet auch für das Neuzeichnen selbst "bitte neu zeichnen". Das darf
        // keine Kette auslösen, sonst zeichnet das Fenster im Leerlauf jedes Bildschirmbild.
        if response.repaint && !matches!(event, WindowEvent::RedrawRequested) {
            r.window.request_redraw();
        }
        match event {
            WindowEvent::CloseRequested => el.exit(),
            WindowEvent::Resized(size) => {
                if size.width > 0 && size.height > 0 {
                    r.config.width = size.width;
                    r.config.height = size.height;
                    r.surface.configure(&r.gpu.device, &r.config);
                }
                r.window.request_redraw();
            }
            WindowEvent::ScaleFactorChanged { .. } => r.window.request_redraw(),
            WindowEvent::RedrawRequested => {
                if let Err(e) = self.redraw() {
                    self.fail(el, e);
                }
            }
            WindowEvent::ModifiersChanged(m) => self.modifiers = m.state(),
            WindowEvent::KeyboardInput { event, .. } if event.state == ElementState::Pressed => {
                let key = event.logical_key.clone();
                self.key(el, &key);
            }
            WindowEvent::MouseWheel { delta, .. } => {
                let notches = match delta {
                    MouseScrollDelta::LineDelta(_, y) => y as f64,
                    MouseScrollDelta::PixelDelta(p) => p.y / 40.0,
                };
                if self.modifiers.control_key() {
                    let z = self.controls.view.zoom * 1.15f64.powf(notches);
                    self.controls.view.zoom = z.clamp(ZOOM_MIN, ZOOM_MAX);
                    self.touch();
                } else {
                    let step = if self.modifiers.shift_key() { WHEEL_STEP_DAYS_SHIFT } else { WHEEL_STEP_DAYS };
                    self.controls.paused = true;
                    let jd = self.controls.jd + notches * step;
                    self.set_time(jd);
                }
            }
            WindowEvent::MouseInput { state, button: MouseButton::Left, .. } => {
                if state == ElementState::Pressed {
                    self.dragging = true;
                    self.drag_moved = false;
                } else {
                    self.dragging = false;
                    if !self.drag_moved {
                        let now = Instant::now();
                        if self.last_click.is_some_and(|t| now - t < Duration::from_millis(400)) {
                            self.controls.view.yaw = 0.0;
                            self.controls.view.pitch = 0.0;
                            self.controls.view.zoom = 1.0;
                            self.last_click = None;
                        } else {
                            self.last_click = Some(now);
                        }
                    }
                }
                self.touch();
            }
            WindowEvent::CursorMoved { position, .. } => {
                let (dx, dy) = (position.x - self.cursor.x, position.y - self.cursor.y);
                self.cursor = position;
                if self.dragging && (dx != 0.0 || dy != 0.0) {
                    self.drag_moved = true;
                    // Ein Mondhalbmesser Mausweg dreht die Kugel um einen Radiant: die Oberfläche folgt dem Zeiger.
                    let (w, h) = (r.config.width.max(1), r.config.height.max(1));
                    let radius_px = (0.5 * w.min(h) as f64 * 0.7 * self.controls.view.zoom).max(40.0);
                    let v = &mut self.controls.view;
                    v.yaw -= dx / radius_px;
                    v.pitch = (v.pitch - dy / radius_px).clamp(-89f64.to_radians(), 89f64.to_radians());
                    self.touch();
                } else {
                    // Zeiger über einer Zeile: Herkunft anzeigen.
                    r.window.request_redraw();
                }
            }
            _ => {}
        }
    }

    fn about_to_wait(&mut self, el: &ActiveEventLoop) {
        let Some(r) = &self.running else { return };
        if self.wants_continuous() {
            // Zeitraffer oder Kamerafahrt: das nächste Bild anfordern und schlafen. Das Fenstersystem
            // liefert die Aufforderung im Takt des Bildschirms; die Schleife dreht dabei nicht leer.
            el.set_control_flow(ControlFlow::Wait);
            r.window.request_redraw();
        } else if self.controls.paused {
            // Nichts bewegt sich: schlafen, bis eine Eingabe kommt.
            el.set_control_flow(ControlFlow::Wait);
        } else {
            // Echtzeit: Der Mond bewegt sich kaum. Einmal je Sekunde genügt.
            el.set_control_flow(ControlFlow::WaitUntil(Instant::now() + Duration::from_secs(1)));
        }
    }
}

pub fn run(session: Session, settings: Settings) -> Result<()> {
    let event_loop = EventLoop::new().map_err(|e| anyhow!("Fenstersystem nicht erreichbar: {e}"))?;
    let controls = Controls::new(&settings);
    // Startzeitpunkt prüfen, bevor ein Fenster aufgeht: außerhalb der Ephemeride gibt es eine Meldung.
    session.scene(settings.site, controls.jd)?;
    let now = Instant::now();
    let mut app = App {
        session,
        settings,
        controls,
        running: None,
        error: None,
        palette: load_palette(),
        palette_stamp: theme_path().and_then(|p| std::fs::metadata(p).ok()).and_then(|m| m.modified().ok()),
        palette_checked: now,
        modifiers: ModifiersState::empty(),
        cursor: PhysicalPosition::new(0.0, 0.0),
        dragging: false,
        drag_moved: false,
        last_click: None,
        last_tick: now,
        active_until: now,
        frame_ms: 0.0,
        frames: 0,
        notice: None,
    };
    event_loop.run_app(&mut app).map_err(|e| anyhow!("Fensterschleife: {e}"))?;
    match app.error {
        Some(e) => Err(e),
        None => Ok(()),
    }
}
