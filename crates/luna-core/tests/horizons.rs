use std::path::PathBuf;

use luna_core::{verify, Ephemeris};

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

#[test]
fn rechenkette_gegen_jpl_horizons() {
    let eph = Ephemeris::load(&root().join("data/raw")).expect("Kerne laden");
    let groups = verify::run(&eph, &root().join("reference/horizons")).expect("Prüfung");
    println!("{}", verify::format_report(&groups));
    for g in &groups {
        assert!(g.epochs >= 8 && g.sites >= 3);
        for c in &g.checks {
            assert!(c.pass(), "{}: {} weicht um {} {} ab (Toleranz {})", g.title, c.name, c.max_dev, c.unit, c.tolerance);
        }
    }
    assert!(groups[0].epochs >= 50);
}
