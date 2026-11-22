//! LUNA Bildentstehung.
//!
//! Der Renderer kennt keine Astronomie: Er bekommt je Bild einen fertigen Satz
//! Zahlen (`FrameParams`) und zeichnet daraus die Mondkugel. Gerechnet wird in
//! physikalischen Helligkeiten; die Belichtung ist der letzte Schritt.

pub mod camera;
pub mod gpu;
pub mod moon;
pub mod photometry;

pub use camera::{Camera, FrameParams, UpMode, View, BASE_FOV_DEG};
pub use gpu::Gpu;
pub use moon::{MoonRenderer, Offscreen, RenderMode, OFFSCREEN_FORMAT};
