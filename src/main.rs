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
    let first = args.next();
    let pen_x11 = first.as_deref() == Some(std::ffi::OsStr::new("--pen-x11"));
    let pen_wayland = first.as_deref() == Some(std::ffi::OsStr::new("--pen-wayland"));
    let explicit_pen = pen_x11 || pen_wayland;
    let pen_flag = if pen_wayland {
        "--pen-wayland"
    } else {
        "--pen-x11"
    };
    let command = if explicit_pen { args.next() } else { first };
    if explicit_pen && command.as_deref().is_some_and(|arg| arg != "--smoke-test") {
        bail!("Use {pen_flag} alone, or {pen_flag} --smoke-test");
    }
    if explicit_pen && args.next().is_some() {
        bail!("Use {pen_flag} alone, or {pen_flag} --smoke-test");
    }
    if command.as_deref() == Some(std::ffi::OsStr::new("--help")) {
        println!(
            "RustyMusicSheets {}\n\n\
                  rustymusicsheets                 Open the desktop application\n\
                  rustymusicsheets --import PATH   Import PDF/PNG/JPEG or a folder recursively\n\
                  rustymusicsheets --list          List scores in the library\n\
                  rustymusicsheets --detect-instruments [ID]  Create instrument proposals\n\
                  rustymusicsheets --omr SOURCE OUTPUT_DIR [--audiveris EXE]  Recognize to MusicXML (Linux)\n\
                  rustymusicsheets --inspect-musicxml FILE  List recognized parts and warnings\n\
                  rustymusicsheets --midi FILE OUTPUT.mid [--parts P1,P2] [--program P1=43]\n\
                    [--tempo-scale 0.8] [--play] [--play-seconds 5] [--soundfont FILE] [--fluidsynth EXE]\n\
                  rustymusicsheets --check         Check PDF and OCR helper programs\n\
                  rustymusicsheets --smoke-test    Open the UI for three seconds\n\n\
                  rustymusicsheets --pen-x11       Open with Linux X11/XWayland pen compatibility\n\
                  rustymusicsheets --pen-wayland   Open with native Wayland tablet-v2 pen input\n\
                  Library/config/cache respect XDG directory variables.",
            env!("CARGO_PKG_VERSION")
        );
        return Ok(());
    }
    if command.as_deref() == Some(std::ffi::OsStr::new("--check")) {
        for tool in ["pdfinfo", "pdftoppm", "pdftotext", "tesseract"] {
            pdf::run(
                std::process::Command::new(tool).arg(if tool == "tesseract" {
                    "--version"
                } else {
                    "-v"
                }),
                &AtomicBool::new(false),
                std::time::Duration::from_secs(5),
            )?;
            println!("{tool}: available");
        }
        return Ok(());
    }
    if let Some(command @ ("--omr" | "--inspect-musicxml" | "--midi")) =
        command.as_deref().and_then(|s| s.to_str())
    {
        return rustymusicsheets::music_cli::run(command, args.collect());
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
        Some("--detect-instruments") => {
            let id = args
                .next()
                .map(|id| id.to_string_lossy().parse::<i64>())
                .transpose()?;
            let scores: Vec<_> = Library::open(&paths)?
                .scores()?
                .into_iter()
                .filter(|score| id.is_none_or(|id| score.id == id))
                .collect();
            if scores.is_empty() {
                bail!("No matching scores to analyze");
            }
            let options = paths.preferences()?.detection_options;
            eprintln!("{}", rustymusicsheets::instruments::DETECTION_TIME_WARNING);
            let job = rustymusicsheets::instruments::job::DetectionJob::start(
                paths.clone(),
                scores,
                options,
                false,
                || {},
            );
            loop {
                let update = job.updates.recv()?;
                if update.done {
                    println!(
                        "Analyzed {} scores; {} failed. Review proposals in Library options.",
                        update.summary.processed, update.summary.failed
                    );
                    for error in &update.summary.errors {
                        eprintln!("{error}");
                    }
                    if update.summary.failed > 0 {
                        bail!("Some instrument detections failed");
                    }
                    break;
                }
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
                    .with_app_id(rustymusicsheets::icon::APP_ID)
                    .with_icon(rustymusicsheets::icon::application_icon()?)
                    .with_inner_size([1280.0, 900.0])
                    .with_min_inner_size([640.0, 360.0]),
                ..Default::default()
            };
            #[cfg(target_os = "linux")]
            let options = if pen_x11 {
                use winit::platform::x11::EventLoopBuilderExtX11;
                let mut options = options;
                options.event_loop_builder = Some(Box::new(|builder| {
                    builder.with_x11();
                }));
                options
            } else if pen_wayland {
                use winit::platform::wayland::EventLoopBuilderExtWayland;
                let mut options = options;
                options.event_loop_builder = Some(Box::new(|builder| {
                    builder.with_wayland();
                }));
                options
            } else {
                options
            };
            #[cfg(not(target_os = "linux"))]
            if explicit_pen {
                bail!("{pen_flag} is available only on Linux");
            }
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
