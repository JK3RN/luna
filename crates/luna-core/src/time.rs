//! Zeitskalen.
//!
//! Intern rechnet LUNA mit `hifitime::Epoch` (gleichförmige Zeit, Schaltsekunden
//! aus der mitgelieferten Tabelle). Nach außen gilt die "bürgerliche" Zeit:
//! ab 1972 UTC, davor UT1 über ein ΔT-Modell (Kategorie N).

use hifitime::Epoch;

/// 1972-01-01 00:00 UTC als Julianisches Datum: Beginn der Schaltsekunden-UTC.
pub const JD_UTC_START: f64 = 2_441_317.5;
const TT_MINUS_TAI_S: f64 = 32.184;

/// ΔT = TT − UT1 in Sekunden nach Espenak & Meeus (NASA Five Millennium Canon),
/// hier nur für den Bereich vor 1972 verwendet. Näherung, Fehler einige Zehntelsekunden.
pub fn delta_t_s(year: f64) -> f64 {
    let y = year;
    if y < 1860.0 {
        let t = y - 1800.0;
        13.72 - 0.332447 * t + 0.0068612 * t.powi(2) + 0.0041116 * t.powi(3)
            - 0.00037436 * t.powi(4)
            + 0.0000121272 * t.powi(5)
            - 0.0000001699 * t.powi(6)
            + 0.000000000875 * t.powi(7)
    } else if y < 1900.0 {
        let t = y - 1860.0;
        7.62 + 0.5737 * t - 0.251754 * t.powi(2) + 0.01680668 * t.powi(3)
            - 0.0004473624 * t.powi(4)
            + t.powi(5) / 233174.0
    } else if y < 1920.0 {
        let t = y - 1900.0;
        -2.79 + 1.494119 * t - 0.0598939 * t.powi(2) + 0.0061966 * t.powi(3) - 0.000197 * t.powi(4)
    } else if y < 1941.0 {
        let t = y - 1920.0;
        21.20 + 0.84493 * t - 0.076100 * t.powi(2) + 0.0020936 * t.powi(3)
    } else if y < 1961.0 {
        let t = y - 1950.0;
        29.07 + 0.407 * t - t.powi(2) / 233.0 + t.powi(3) / 2547.0
    } else {
        let t = y - 1975.0;
        45.45 + 1.067 * t - t.powi(2) / 260.0 - t.powi(3) / 718.0
    }
}

fn year_of_jd(jd: f64) -> f64 {
    2000.0 + (jd - 2_451_544.5) / 365.2425
}

/// Bürgerliches Julianisches Datum (ab 1972 UTC, davor UT1) → Epoche.
pub fn epoch_from_civil_jd(jd: f64) -> Epoch {
    if jd >= JD_UTC_START {
        Epoch::from_jde_utc(jd)
    } else {
        let jd_tt = jd + delta_t_s(year_of_jd(jd)) / 86400.0;
        Epoch::from_jde_tai(jd_tt - TT_MINUS_TAI_S / 86400.0)
    }
}

/// Epoche → bürgerliches Julianisches Datum (ab 1972 UTC, davor UT1).
pub fn civil_jd(epoch: Epoch) -> f64 {
    let jd_utc = epoch.to_jde_utc_days();
    if jd_utc >= JD_UTC_START {
        return jd_utc;
    }
    let jd_tt = epoch.to_jde_tt_days();
    let mut jd = jd_tt - delta_t_s(year_of_jd(jd_tt)) / 86400.0;
    jd = jd_tt - delta_t_s(year_of_jd(jd)) / 86400.0;
    jd
}

/// UT1 als Julianisches Datum für das Ausweichmodell der Erddrehung:
/// ab 1972 wird UT1 = UTC gesetzt (Fehler ≤ 0,9 s), davor gilt das ΔT-Modell.
pub fn ut1_jd_model(epoch: Epoch) -> f64 {
    civil_jd(epoch)
}

/// Gregorianisches Kalenderdatum aus einem Julianischen Datum.
/// Rückgabe: (Jahr, Monat, Tag, Stunde, Minute, Sekunde).
pub fn calendar_from_jd(jd: f64) -> (i32, u32, u32, u32, u32, f64) {
    let z = (jd + 0.5).floor();
    let f = jd + 0.5 - z;
    let alpha = ((z - 1_867_216.25) / 36_524.25).floor();
    let a = z + 1.0 + alpha - (alpha / 4.0).floor();
    let b = a + 1524.0;
    let c = ((b - 122.1) / 365.25).floor();
    let d = (365.25 * c).floor();
    let e = ((b - d) / 30.6001).floor();
    let day = b - d - (30.6001 * e).floor();
    let month = if e < 14.0 { e - 1.0 } else { e - 13.0 };
    let year = if month > 2.0 { c - 4716.0 } else { c - 4715.0 };
    let secs = f * 86400.0;
    let h = (secs / 3600.0).floor();
    let m = ((secs - h * 3600.0) / 60.0).floor();
    let s = secs - h * 3600.0 - m * 60.0;
    (year as i32, month as u32, day as u32, h as u32, m as u32, s)
}

/// Julianisches Datum aus gregorianischem Kalenderdatum.
pub fn jd_from_calendar(year: i32, month: u32, day: u32, h: u32, mi: u32, s: f64) -> f64 {
    let (mut y, mut m) = (year as f64, month as f64);
    if m <= 2.0 {
        y -= 1.0;
        m += 12.0;
    }
    let a = (y / 100.0).floor();
    let b = 2.0 - a + (a / 4.0).floor();
    (365.25 * (y + 4716.0)).floor() + (30.6001 * (m + 1.0)).floor() + day as f64 + b - 1524.5
        + (h as f64 + mi as f64 / 60.0 + s / 3600.0) / 24.0
}

/// Liest `JJJJ-MM-TT`, `JJJJ-MM-TTThh:mm[:ss[.fff]]` (optional mit `Z`) oder `JD<zahl>`.
/// Die Angabe gilt als UTC (vor 1972 als UT1).
pub fn parse_civil(s: &str) -> Result<f64, String> {
    let s = s.trim();
    if let Some(rest) = s.strip_prefix("JD").or_else(|| s.strip_prefix("jd")) {
        let jd = rest.trim().parse::<f64>().map_err(|e| format!("ungültiges Julianisches Datum: {e}"))?;
        if !jd.is_finite() {
            return Err(format!("ungültiges Julianisches Datum: '{}'", rest.trim()));
        }
        return Ok(jd);
    }
    let s = s.trim_end_matches('Z').trim_end_matches(" UTC");
    let (date, time) = match s.split_once(['T', ' ']) {
        Some((d, t)) => (d, Some(t)),
        None => (s, None),
    };
    let dp: Vec<&str> = date.split('-').collect();
    if dp.len() != 3 {
        return Err(format!("Datum nicht lesbar: '{date}' (erwartet JJJJ-MM-TT)"));
    }
    let y: i32 = dp[0].parse().map_err(|_| format!("Jahr nicht lesbar: '{}'", dp[0]))?;
    let mo: u32 = dp[1].parse().map_err(|_| format!("Monat nicht lesbar: '{}'", dp[1]))?;
    let d: u32 = dp[2].parse().map_err(|_| format!("Tag nicht lesbar: '{}'", dp[2]))?;
    if !(1..=12).contains(&mo) || !(1..=31).contains(&d) {
        return Err(format!("Datum außerhalb des Kalenders: '{date}'"));
    }
    let (mut h, mut mi, mut sec) = (0u32, 0u32, 0f64);
    if let Some(t) = time {
        let tp: Vec<&str> = t.split(':').collect();
        if tp.len() < 2 || tp.len() > 3 {
            return Err(format!("Uhrzeit nicht lesbar: '{t}' (erwartet hh:mm[:ss])"));
        }
        h = tp[0].parse().map_err(|_| format!("Stunde nicht lesbar: '{}'", tp[0]))?;
        mi = tp[1].parse().map_err(|_| format!("Minute nicht lesbar: '{}'", tp[1]))?;
        if tp.len() == 3 {
            sec = tp[2].parse().map_err(|_| format!("Sekunde nicht lesbar: '{}'", tp[2]))?;
        }
        if h > 23 || mi > 59 || !(0.0..61.0).contains(&sec) {
            return Err(format!("Uhrzeit außerhalb des Tages: '{t}'"));
        }
    }
    Ok(jd_from_calendar(y, mo, d, h, mi, sec))
}

/// Formatiert ein bürgerliches Julianisches Datum als `JJJJ-MM-TT hh:mm:ss`.
pub fn format_civil(jd: f64) -> String {
    // Auf ganze Sekunden runden, damit nie "…:60" erscheint.
    let jd_r = (jd * 86400.0).round() / 86400.0;
    let (y, mo, d, h, mi, s) = calendar_from_jd(jd_r + 1e-9);
    format!("{y:04}-{mo:02}-{d:02} {h:02}:{mi:02}:{:02}", s.floor() as u32)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn calendar_roundtrip() {
        let jd = jd_from_calendar(2026, 11, 15, 8, 30, 15.0);
        let (y, mo, d, h, mi, s) = calendar_from_jd(jd + 1e-9);
        assert_eq!((y, mo, d, h, mi), (2026, 11, 15, 8, 30));
        assert!((s - 15.0).abs() < 1e-3);
        assert!((jd_from_calendar(2000, 1, 1, 12, 0, 0.0) - 2_451_545.0).abs() < 1e-9);
    }

    #[test]
    fn civil_epoch_roundtrip() {
        for jd in [2_417_000.271_3, 2_435_300.904_1, 2_441_000.1, 2_447_300.201_4, 2_461_360.5] {
            let back = civil_jd(epoch_from_civil_jd(jd));
            assert!((back - jd).abs() * 86400.0 < 2e-3, "jd {jd}: {back}");
        }
    }

    #[test]
    fn delta_t_is_continuous() {
        for y in [1860.0, 1900.0, 1920.0, 1941.0, 1961.0] {
            let jump: f64 = delta_t_s(y - 1e-6) - delta_t_s(y + 1e-6);
            assert!(jump.abs() < 0.2, "Sprung bei {y}: {jump}");
        }
        assert!((delta_t_s(1900.0) + 2.79).abs() < 1e-9);
        assert!((delta_t_s(1972.0) - 42.2).abs() < 0.3);
    }

    #[test]
    fn parse_forms() {
        assert!((parse_civil("2000-01-01T12:00:00Z").unwrap() - 2_451_545.0).abs() < 1e-9);
        assert!((parse_civil("2000-01-01 12:00").unwrap() - 2_451_545.0).abs() < 1e-9);
        assert!((parse_civil("JD2451545.0").unwrap() - 2_451_545.0).abs() < 1e-9);
        assert!(parse_civil("JDnan").is_err());
        assert!(parse_civil("JDinf").is_err());
        assert!(parse_civil("gestern").is_err());
        assert!(parse_civil("2026-13-01").is_err());
    }
}
