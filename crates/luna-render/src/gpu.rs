//! Grafikkarte auswählen und öffnen.
//!
//! Auf dem Zielgerät liegen mehrere Vulkan-Treibereinträge. LUNA wählt die Karte
//! deshalb ausdrücklich aus und nennt sie beim Start, statt der Reihenfolge des
//! Systems zu vertrauen.

use anyhow::{anyhow, bail, Result};

pub struct Gpu {
    pub instance: wgpu::Instance,
    pub adapter: wgpu::Adapter,
    pub device: wgpu::Device,
    pub queue: wgpu::Queue,
    pub info: wgpu::AdapterInfo,
}

fn rank(info: &wgpu::AdapterInfo) -> i32 {
    let name = info.name.to_lowercase();
    let amd = name.contains("radv") || name.contains("amd") || name.contains("radeon");
    match info.device_type {
        wgpu::DeviceType::IntegratedGpu | wgpu::DeviceType::DiscreteGpu if amd => 3,
        wgpu::DeviceType::DiscreteGpu => 2,
        wgpu::DeviceType::IntegratedGpu => 2,
        wgpu::DeviceType::VirtualGpu => 1,
        // Software-Renderer (llvmpipe) und Unbekanntes nur, wenn ausdrücklich verlangt.
        _ => 0,
    }
}

impl Gpu {
    /// `instance`: mit oder ohne Anzeige erzeugt. `surface`: falls in ein Fenster gezeichnet wird.
    /// Die Umgebungsvariable `LUNA_ADAPTER` wählt eine Karte über einen Teil ihres Namens.
    pub async fn new(instance: wgpu::Instance, surface: Option<&wgpu::Surface<'_>>) -> Result<Self> {
        let wanted = std::env::var("LUNA_ADAPTER").ok().map(|s| s.to_lowercase());
        let adapters = instance.enumerate_adapters(wgpu::Backends::VULKAN).await;
        if adapters.is_empty() {
            bail!("Kein Vulkan-Gerät gefunden. LUNA braucht einen Vulkan-Treiber (etwa Mesa RADV, Mesa ANV oder den NVIDIA-Treiber).");
        }
        let mut best: Option<(i32, wgpu::Adapter)> = None;
        let mut seen = Vec::new();
        for a in adapters {
            let info = a.get_info();
            seen.push(format!("{} ({:?})", info.name, info.device_type));
            if let Some(s) = surface {
                if !a.is_surface_supported(s) {
                    continue;
                }
            }
            let r = match &wanted {
                Some(w) => {
                    if info.name.to_lowercase().contains(w) {
                        10
                    } else {
                        -1
                    }
                }
                None => rank(&info),
            };
            if r > 0 && best.as_ref().is_none_or(|(br, _)| r > *br) {
                best = Some((r, a));
            }
        }
        let (_, adapter) = best.ok_or_else(|| {
            anyhow!(
                "Keine geeignete Grafikkarte gefunden. Gesehen: {}. Mit LUNA_ADAPTER=<Namensteil> lässt sich eine Karte erzwingen.",
                seen.join("; ")
            )
        })?;
        let info = adapter.get_info();

        let limits = wgpu::Limits::default();
        let supported = adapter.limits();
        if supported.max_texture_dimension_2d < 8192 {
            bail!(
                "Die Grafikkarte {} erlaubt nur Texturen bis {} Bildpunkte; LUNA braucht 8192.",
                info.name,
                supported.max_texture_dimension_2d
            );
        }
        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor {
                label: Some("luna"),
                required_features: wgpu::Features::empty(),
                required_limits: limits,
                ..Default::default()
            })
            .await
            .map_err(|e| anyhow!("Grafikgerät ließ sich nicht öffnen: {e}"))?;
        Ok(Self { instance, adapter, device, queue, info })
    }

    /// Ohne Fenster, für Bildtests und Belege.
    pub fn headless() -> Result<Self> {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
            backends: wgpu::Backends::VULKAN,
            ..wgpu::InstanceDescriptor::new_without_display_handle()
        });
        pollster::block_on(Self::new(instance, None))
    }

    pub fn describe(&self) -> String {
        format!("{} · {} · Treiber {}", self.info.name, self.info.backend, self.info.driver_info)
    }
}
