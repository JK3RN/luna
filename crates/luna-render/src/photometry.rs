//! Helligkeiten: Streuverhalten des Mondstaubs und Belichtung.
//!
//! Alles hier ist Kategorie N (Näherung) und so in der Oberfläche gekennzeichnet.

/// Beleuchtungsstärke der Sonne in 1 AE außerhalb der Atmosphäre, in Lux.
/// Literaturwerte liegen zwischen 128 000 und 134 000 lx.
pub const SOLAR_ILLUMINANCE_LX: f64 = 133_100.0;
/// Mittlere Normalalbedo des Mondes, auf die die Farbkarte skaliert wird.
pub const MEAN_NORMAL_ALBEDO: f64 = 0.12;
/// Jenseits dieses Phasenwinkels (schmale Sichel) gilt die Helligkeitskurve nicht mehr:
/// das Verhältnis der beiden Kurven würde wieder ansteigen. Der Wert wird dort gehalten.
pub const PHASE_LIMIT_DEG: f64 = 150.0;

/// Integrale Phasenkurve des Mondes nach Allen (Astrophysical Quantities):
/// Δm = 0,026·|α| + 4·10⁻⁹·α⁴, α in Grad. Gibt den Helligkeitsfaktor relativ zu α = 0.
fn allen_phase(alpha_deg: f64) -> f64 {
    let a = alpha_deg.abs();
    10f64.powf(-0.4 * (0.026 * a + 4.0e-9 * a.powi(4)))
}

/// Integrale Phasenkurve einer Lommel-Seeliger-Kugel, relativ zu α = 0.
fn lommel_seeliger_sphere(alpha_deg: f64) -> f64 {
    let a = alpha_deg.abs().to_radians();
    if a < 1e-6 {
        return 1.0;
    }
    let half = a / 2.0;
    1.0 - half.sin() * half.tan() * (1.0 / (a / 4.0).tan()).ln()
}

/// Phasenfunktion der Oberfläche f(α)/f(0).
///
/// So gewählt, dass eine Kugel mit Lommel-Seeliger-Streuung insgesamt genau der
/// beobachteten Helligkeitskurve des Mondes folgt. Die Lommel-Seeliger-Streuung selbst
/// sorgt dafür, dass der Vollmond bis zum Rand gleich hell ist (kein Randabfall).
pub fn surface_phase_function(alpha_deg: f64) -> f64 {
    let a = alpha_deg.abs().min(PHASE_LIMIT_DEG);
    allen_phase(a) / lommel_seeliger_sphere(a)
}

/// Sättigungsleuchtdichte einer Kamera bei Lichtwert `ev100` (ISO 100), in cd/m².
/// Entspricht der sättigungsbezogenen Empfindlichkeit nach ISO 12232.
pub fn saturation_luminance(ev100: f64) -> f64 {
    1.2 * 2f64.powf(ev100)
}

/// Lichtwert, bei dem die hellsten Flächen der Scheibe knapp unter der Sättigung liegen.
pub fn auto_ev100(alpha_deg: f64, sun_distance_au: f64) -> f64 {
    let e = SOLAR_ILLUMINANCE_LX / (sun_distance_au * sun_distance_au);
    // Helle Hochländer erreichen etwa das 1,7-Fache der mittleren Albedo; bei schmaler
    // Sichel hebt die Lommel-Seeliger-Streuung den hellen Rand bis auf das Doppelte.
    let limb_gain = 1.0 + 0.5 * (1.0 - alpha_deg.to_radians().cos());
    let peak = e / std::f64::consts::PI * MEAN_NORMAL_ALBEDO * 1.7 * surface_phase_function(alpha_deg) * limb_gain;
    (peak / 0.92 / 1.2).log2()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn phase_function_is_sane() {
        assert!((surface_phase_function(0.0) - 1.0).abs() < 1e-9);
        let mut last = 1.0;
        for a in (1..=PHASE_LIMIT_DEG as i32).step_by(1) {
            let f = surface_phase_function(a as f64);
            assert!(f > 0.0 && f < last, "f({a}) = {f} fällt nicht");
            last = f;
        }
        // Jenseits der Grenze bleibt der Wert stehen.
        assert_eq!(surface_phase_function(175.0), surface_phase_function(PHASE_LIMIT_DEG));
        // Stichproben, von Hand nachgerechnet.
        assert!((surface_phase_function(90.0) - 0.242).abs() < 0.005);
        assert!((lommel_seeliger_sphere(90.0) - 0.3768).abs() < 0.001);
    }

    #[test]
    fn exposure_matches_looney_11() {
        // Faustregel der Fotografen für den Vollmond: Blende 11, 1/ISO s → EV100 ≈ 13,6 bei mittlerem Grau.
        // Auf die Lichter belichtet liegt die Automatik etwa eine Stufe darunter.
        let ev = auto_ev100(5.0, 1.0);
        assert!((12.0..14.0).contains(&ev), "EV {ev}");
    }
}
