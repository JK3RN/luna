//! Führt `luna --verify` aus: Daten, Rechenkette, Kartenlage und Bildprüfungen.
//! Braucht die geladenen und aufbereiteten Daten sowie eine Vulkan-Grafikkarte.

use std::process::Command;

#[test]
fn selbstpruefung_besteht() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let out = Command::new(env!("CARGO_BIN_EXE_luna")).arg("--verify").current_dir(&root).output().expect("luna starten");
    let text = String::from_utf8_lossy(&out.stdout);
    println!("{text}{}", String::from_utf8_lossy(&out.stderr));
    assert!(out.status.success(), "luna --verify meldet Fehler");
    assert!(text.contains("ERGEBNIS: alle Prüfungen bestanden."));
    assert!(!text.contains("VERFEHLT"));
}

#[test]
fn falsche_eingaben_werden_klar_abgelehnt() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let run = |args: &[&str]| {
        let out = Command::new(env!("CARGO_BIN_EXE_luna")).args(args).current_dir(&root).output().expect("luna starten");
        (out.status.code(), String::from_utf8_lossy(&out.stderr).to_string())
    };
    // Zeitpunkt außerhalb der Ephemeride: klare Meldung statt eines falschen Bildes.
    let (code, err) = run(&["--info", "--at", "1700-01-01"]);
    assert_eq!(code, Some(2));
    assert!(err.contains("außerhalb der Ephemeride"), "{err}");
    let (code, err) = run(&["--info", "--lat", "95"]);
    assert_eq!(code, Some(2));
    assert!(err.contains("Breite"), "{err}");
    let (code, err) = run(&["--at", "gestern", "--info"]);
    assert_eq!(code, Some(2));
    assert!(err.contains("--at"), "{err}");
    let (code, _) = run(&["--kaputt"]);
    assert_eq!(code, Some(2));
    // Nicht endliche Zahlen werden abgelehnt statt ein leeres Bild zu rechnen.
    for (flag, v) in [("--zoom", "nan"), ("--ev", "inf"), ("--scale", "NaN"), ("--yaw", "nan"), ("--pitch", "-inf"), ("--at", "JDnan")] {
        let (code, err) = run(&["--info", flag, v]);
        assert_eq!(code, Some(2), "{flag} {v}: {err}");
        assert!(err.contains(flag), "{flag} {v}: {err}");
    }
    // Pole und Mond unter dem Horizont rechnen ohne Fehler.
    for args in [["--info", "--lat", "90", "--lon", "0"], ["--info", "--lat", "-90", "--lon", "0"]] {
        assert_eq!(run(&args).0, Some(0));
    }
}
