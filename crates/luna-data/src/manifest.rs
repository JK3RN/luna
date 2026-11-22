//! Manifest: hält je Datei Quelle, Abrufdatum, SHA-256 und Verarbeitungsschritte fest.
//! LUNA prüft es bei jedem Start.

use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};

use anyhow::{anyhow, bail, Context, Result};
use rayon::prelude::*;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ManifestEntry {
    /// Pfad relativ zum Datenordner.
    pub path: String,
    pub sha256: String,
    pub bytes: u64,
    /// Herkunft: Adresse der Originaldatei oder "luna-forge".
    pub source: String,
    /// Produktname und Nennung, wie die Quelle sie erbittet.
    pub product: String,
    /// Abruf- bzw. Erzeugungszeit (UTC).
    pub retrieved_utc: String,
    /// Verarbeitungsschritte in Klartext.
    pub steps: Vec<String>,
}

/// Kennzahlen aus der Aufbereitung, die der Renderer braucht.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ForgeInfo {
    pub forge_version: String,
    /// Flächengewichteter Mittelwert der linearisierten Farbkarte (Leuchtdichte, 0…1).
    pub albedo_mean_linear: f64,
    /// Kodierung der Horizontkarten: Winkel = sign(x)·x²·max, x = 2·Wert − 1.
    pub horizon_max_angle_deg: f64,
    pub horizon_directions: u32,
    /// Größte Suchweite der Horizontberechnung in km.
    pub horizon_reach_km: f64,
    pub height_min_m: f64,
    pub height_max_m: f64,
    /// Kantenlänge eines Höhentexels am Äquator in km.
    pub height_texel_km: f64,
    pub albedo_texel_km: f64,
    /// Ergebnisse der Kartenlage-Prüfungen in Klartext.
    pub checks: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Manifest {
    pub schema: u32,
    pub raw: Vec<ManifestEntry>,
    pub forged: Vec<ManifestEntry>,
    pub forge: ForgeInfo,
}

/// Wo die Daten liegen.
#[derive(Clone, Debug)]
pub struct DataPaths {
    pub root: PathBuf,
}

impl DataPaths {
    /// Sucht den Datenordner: `LUNA_DATA`, sonst `data/` neben dem Arbeitsverzeichnis
    /// oder neben dem Programm (bis zu vier Ebenen aufwärts).
    pub fn locate() -> Result<Self> {
        if let Ok(p) = std::env::var("LUNA_DATA") {
            let root = PathBuf::from(p);
            if root.join("raw").is_dir() {
                return Ok(Self { root });
            }
            bail!("LUNA_DATA zeigt auf {}, dort fehlt der Ordner raw/", root.display());
        }
        let mut starts = vec![std::env::current_dir()?];
        if let Ok(exe) = std::env::current_exe() {
            if let Some(d) = exe.parent() {
                starts.push(d.to_path_buf());
            }
        }
        for s in starts {
            let mut d = Some(s.as_path());
            for _ in 0..5 {
                let Some(dir) = d else { break };
                if dir.join("data/raw").is_dir() {
                    return Ok(Self { root: dir.join("data") });
                }
                d = dir.parent();
            }
        }
        bail!("Datenordner nicht gefunden. Bitte im Projektordner starten oder LUNA_DATA setzen; die Daten lädt scripts/fetch-data.sh.")
    }
    pub fn raw(&self) -> PathBuf {
        self.root.join("raw")
    }
    pub fn forged(&self) -> PathBuf {
        self.root.join("forged")
    }
    pub fn manifest(&self) -> PathBuf {
        self.root.join("manifest.json")
    }
    /// Ordner mit den eingefrorenen Referenzwerten.
    pub fn reference(&self) -> PathBuf {
        self.root.parent().unwrap_or(Path::new(".")).join("reference")
    }
}

pub fn sha256_file(path: &Path) -> Result<(String, u64)> {
    let mut f = File::open(path).with_context(|| format!("{} nicht lesbar", path.display()))?;
    let mut h = Sha256::new();
    let mut buf = vec![0u8; 1 << 20];
    let mut total = 0u64;
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
        total += n as u64;
    }
    let digest = h.finalize();
    Ok((digest.iter().map(|b| format!("{b:02x}")).collect(), total))
}

impl Manifest {
    pub fn load(paths: &DataPaths) -> Result<Self> {
        let p = paths.manifest();
        let text = std::fs::read_to_string(&p)
            .with_context(|| format!("Manifest {} fehlt — bitte luna-forge ausführen", p.display()))?;
        let m: Manifest = serde_json::from_str(&text).with_context(|| format!("Manifest {} nicht lesbar", p.display()))?;
        if m.schema != 1 {
            bail!("Manifest-Schema {} wird nicht unterstützt", m.schema);
        }
        Ok(m)
    }

    pub fn save(&self, paths: &DataPaths) -> Result<()> {
        std::fs::write(paths.manifest(), serde_json::to_string_pretty(self)? + "\n")?;
        Ok(())
    }

    /// Prüft alle Dateien gegen ihre SHA-256-Summen. Liefert die Zahl der geprüften Byte.
    pub fn verify(&self, paths: &DataPaths) -> Result<u64> {
        let all: Vec<&ManifestEntry> = self.raw.iter().chain(self.forged.iter()).collect();
        let results: Vec<Result<u64>> = all
            .par_iter()
            .map(|e| {
                let (sum, bytes) = sha256_file(&paths.root.join(&e.path))?;
                if sum != e.sha256 {
                    return Err(anyhow!(
                        "Prüfsumme von {} stimmt nicht mit dem Manifest überein (Datei verändert oder beschädigt)",
                        e.path
                    ));
                }
                Ok(bytes)
            })
            .collect();
        let mut total = 0;
        for r in results {
            total += r?;
        }
        Ok(total)
    }

    pub fn entry(&self, path: &str) -> Option<&ManifestEntry> {
        self.raw.iter().chain(self.forged.iter()).find(|e| e.path == path)
    }
}
