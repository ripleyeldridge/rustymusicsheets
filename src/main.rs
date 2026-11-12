use anyhow::{bail, Result};
use rustymusicsheets::{app::MusicApp, import, model::Paths, pdf, storage::Library};
use std::{path::PathBuf, sync::atomic::AtomicBool};

fn main() {
    if let Err(error) = run() {
        eprintln!("RustyMusicSheets: {error:#}");
        std::process::exit(1);
    }
}

fn run() -> Result<()> {
    let mut args = std::env::args_os().skip(1);
    let command = args.next();
    if command.as_deref() == Some(std::ffi::OsStr::new("--help")) {
        println!(
            "RustyMusicSheets 0.1\n\n\
                  rustymusicsheets                 Open the desktop application\n\
                  rustymusicsheets --import PATH   Import a PDF or folder recursively\n\
                  rustymusicsheets --list          List scores in the library\n\
                  rustymusicsheets --check         Check PDF helper programs\n\
                  rustymusicsheets --smoke-test    Open the UI for three seconds\n\n\
                  Library/config/cache respect XDG directory variables."
        );
        return Ok(());
    }
    if command.as_deref() == Some(std::ffi::OsStr::new("--check")) {
        for tool in ["pdfinfo", "pdftoppm"] {
            pdf::run(
                std::process::Command::new(tool).arg("-v"),
                &AtomicBool::new(false),
                std::time::Duration::from_secs(5),
            )?;
            println!("{tool}: available");
        }
        return Ok(());
    }
    let paths = Paths::discover()?;
    match command.as_deref().and_then(|s| s.to_str()) {
        Some("--import") => {
            let source = args
                .next()
                .map(PathBuf::from)
                .ok_or_else(|| anyhow::anyhow!("Usage: rustymusicsheets --import PATH"))?;
            let report = import::batch(&[source], true, &paths, &AtomicBool::new(false), |_| {})?;
            println!(
                "{} imported, {} duplicates, {} failed",
                report.imported, report.skipped, report.failed
            );
            for error in &report.errors {
                eprintln!("{error}");
            }
            if report.failed > 0 {
                bail!("Some files could not be imported");
            }
        }
        Some("--list") => {
            for score in Library::open(&paths)?.scores()? {
                println!("{}\t{} pages\t{}", score.id, score.pages, score.title);
            }
        }
        None | Some("--smoke-test") => {
            let smoke = command.is_some().then_some(3);
            let options = eframe::NativeOptions {
                viewport: eframe::egui::ViewportBuilder::default()
                    .with_title("RustyMusicSheets")
                    .with_inner_size([1280.0, 900.0])
                    .with_min_inner_size([800.0, 600.0]),
                ..Default::default()
            };
            eframe::run_native(
                "RustyMusicSheets",
                options,
                Box::new(move |cc| Ok(Box::new(MusicApp::new(cc, paths, smoke)?))),
            )
            .map_err(|e| anyhow::anyhow!("{e}"))?;
        }
        Some(other) => bail!("Unknown argument {other}; use --help"),
    }
    Ok(())
}
