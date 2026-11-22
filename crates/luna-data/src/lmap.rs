//! `.lmap`: eine Karte in Plattkartenprojektion mit fertiger Verkleinerungskette.
//!
//! Aufbau: 32 Byte Kopf (Kennung "LMAP", Version, Breite, Höhe, Format, Anzahl Stufen,
//! zwei Reservefelder; alles u32 little-endian), danach die Stufen von groß nach klein.
//! Spalte 0 liegt bei −180° Länge, Zeile 0 am Nordpol; die Bildmitte ist 0°/0°.

use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::Path;

use anyhow::{bail, Context, Result};
use memmap2::Mmap;

const MAGIC: &[u8; 4] = b"LMAP";
const VERSION: u32 = 1;
const HEADER: usize = 32;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LmapFormat {
    /// 4 × 8 Bit, sRGB-kodierte Farbe.
    Rgba8Srgb = 1,
    /// 4 × 8 Bit, lineare Werte.
    Rgba8 = 2,
    /// 2 × 16 Bit Gleitkomma.
    Rg16Float = 3,
}

impl LmapFormat {
    pub fn bytes_per_texel(self) -> usize {
        4
    }
    fn from_u32(v: u32) -> Result<Self> {
        Ok(match v {
            1 => LmapFormat::Rgba8Srgb,
            2 => LmapFormat::Rgba8,
            3 => LmapFormat::Rg16Float,
            _ => bail!("unbekanntes Kartenformat {v}"),
        })
    }
}

/// Größe der Stufe `level`.
pub fn level_size(width: u32, height: u32, level: u32) -> (u32, u32) {
    ((width >> level).max(1), (height >> level).max(1))
}

/// Wie viele Stufen sich durch sauberes Halbieren bilden lassen (mindestens 64 Texel breit).
pub fn mip_count(width: u32, height: u32) -> u32 {
    let mut n = 1;
    let (mut w, mut h) = (width, height);
    while w % 2 == 0 && h % 2 == 0 && w / 2 >= 64 {
        w /= 2;
        h /= 2;
        n += 1;
    }
    n
}

pub fn write(path: &Path, width: u32, height: u32, format: LmapFormat, levels: &[Vec<u8>]) -> Result<()> {
    for (i, l) in levels.iter().enumerate() {
        let (w, h) = level_size(width, height, i as u32);
        if l.len() != w as usize * h as usize * format.bytes_per_texel() {
            bail!("Stufe {i} hat {} Byte, erwartet {}×{}×{}", l.len(), w, h, format.bytes_per_texel());
        }
    }
    let f = File::create(path).with_context(|| format!("{} nicht schreibbar", path.display()))?;
    let mut w = BufWriter::with_capacity(1 << 20, f);
    w.write_all(MAGIC)?;
    for v in [VERSION, width, height, format as u32, levels.len() as u32, 0, 0] {
        w.write_all(&v.to_le_bytes())?;
    }
    for l in levels {
        w.write_all(l)?;
    }
    w.flush()?;
    Ok(())
}

/// Eine geöffnete Karte; die Daten bleiben in der Datei und werden nur eingeblendet.
pub struct Lmap {
    map: Mmap,
    pub width: u32,
    pub height: u32,
    pub format: LmapFormat,
    pub levels: u32,
}

impl Lmap {
    pub fn open(path: &Path) -> Result<Self> {
        let f = File::open(path).with_context(|| format!("Karte {} fehlt — bitte luna-forge ausführen", path.display()))?;
        // SAFETY: Die Datei wird nur gelesen; LUNA verändert aufbereitete Karten zur Laufzeit nicht.
        let map = unsafe { Mmap::map(&f)? };
        if map.len() < HEADER || &map[0..4] != MAGIC {
            bail!("{} ist keine LMAP-Datei", path.display());
        }
        let u = |i: usize| u32::from_le_bytes(map[4 + i * 4..8 + i * 4].try_into().unwrap());
        if u(0) != VERSION {
            bail!("{}: Version {} wird nicht unterstützt", path.display(), u(0));
        }
        let (width, height, format, levels) = (u(1), u(2), LmapFormat::from_u32(u(3))?, u(4));
        let mut expect = HEADER;
        for l in 0..levels {
            let (w, h) = level_size(width, height, l);
            expect += w as usize * h as usize * format.bytes_per_texel();
        }
        if map.len() != expect {
            bail!("{}: Dateigröße {} passt nicht zum Kopf (erwartet {expect})", path.display(), map.len());
        }
        Ok(Self { map, width, height, format, levels })
    }

    pub fn level(&self, level: u32) -> &[u8] {
        let mut off = HEADER;
        for l in 0..level {
            let (w, h) = level_size(self.width, self.height, l);
            off += w as usize * h as usize * self.format.bytes_per_texel();
        }
        let (w, h) = level_size(self.width, self.height, level);
        &self.map[off..off + w as usize * h as usize * self.format.bytes_per_texel()]
    }
}
