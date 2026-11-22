//! LUNA — der Mond von jetzt, von hier, mit berechnetem Licht auf gemessenem Relief.

mod app;
mod headless;
mod hudpaint;
mod session;

use anyhow::Result;
use session::{Command, Session, Settings};

fn run() -> Result<bool> {
    let settings = Settings::from_args(std::env::args().skip(1))?;
    if settings.command == Command::Help {
        print!("{}", session::HELP);
        return Ok(true);
    }
    let session = Session::load()?;
    match &settings.command {
        Command::Help => unreachable!(),
        Command::Verify => headless::verify(&session),
        Command::Render(path) => headless::render(&session, &settings, path).map(|_| true),
        Command::Bench => headless::bench(&session, &settings).map(|_| true),
        Command::Info => headless::info(&session, &settings).map(|_| true),
        Command::Window => app::run(session, settings).map(|_| true),
    }
}

fn main() {
    match run() {
        Ok(true) => {}
        Ok(false) => std::process::exit(1),
        Err(e) => {
            eprintln!("LUNA: {e:#}");
            std::process::exit(2);
        }
    }
}
