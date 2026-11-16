use crate::{folder, music, notation, omr};
use anyhow::{bail, Context, Result};
use std::{
    ffi::OsString,
    path::PathBuf,
    sync::atomic::AtomicBool,
    time::{Duration, Instant},
};

pub fn run(command: &str, arguments: Vec<OsString>) -> Result<()> {
    let mut args = arguments.into_iter();
    let input = PathBuf::from(args.next().context("A source filename is required")?);
    if command == "--inspect-musicxml" {
        anyhow::ensure!(args.next().is_none(), "Usage: --inspect-musicxml FILE");
        let composition = notation::load(&input)?;
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "title": composition.title,
                "duration_ticks": composition.duration_ticks,
                "ticks_per_quarter": notation::TICKS_PER_QUARTER,
                "warnings": composition.warnings,
            "source_pages": composition.page_map.as_ref().map(|map|map.source_pages.iter().map(|p|p+1).collect::<Vec<_>>()),
            "page_turns": composition.page_map.as_ref().map(|map|map.turns.iter().map(|turn|
                serde_json::json!({"tick":turn.tick,"source_page":turn.page+1})).collect::<Vec<_>>()),
                "tempos": composition.tempos.iter().map(|t| serde_json::json!({"tick":t.tick,"bpm":t.bpm})).collect::<Vec<_>>(),
                "parts": composition.parts.iter().map(|p| serde_json::json!({
                    "id":p.id,"name":p.name,"staves":p.staves,
                    "gm_instrument":u16::from(p.program)+1,"notes":p.notes.len()
                })).collect::<Vec<_>>()
            }))?
        );
        return Ok(());
    }
    let output = PathBuf::from(
        args.next()
            .context("A destination filename/directory is required")?,
    );
    let mut settings = omr::Settings::default();
    let mut options = notation::MidiOptions::default();
    let mut play = false;
    let mut play_seconds = None;
    while let Some(option) = args.next() {
        let option = option.to_str().context("Option names must be UTF-8")?;
        if option == "--play" {
            anyhow::ensure!(command == "--midi", "--play applies to --midi");
            play = true;
            continue;
        }
        let value = args
            .next()
            .with_context(|| format!("Missing value for {option}"))?;
        let text = || value.to_str().context("This option requires a UTF-8 value");
        match option {
            "--audiveris" if command == "--omr" => settings.audiveris_executable = text()?.into(),
            "--fluidsynth" if command == "--midi" => {
                settings.fluidsynth_executable = text()?.into()
            }
            "--soundfont" if command == "--midi" => settings.soundfont = text()?.into(),
            "--parts" if command == "--midi" => {
                options.selected_parts = Some(text()?.split(',').map(str::to_owned).collect());
            }
            "--program" if command == "--midi" => {
                let (part, program) = text()?
                    .split_once('=')
                    .context("Use --program PART_ID=GM_NUMBER (1–128)")?;
                let program: u16 = program.parse()?;
                anyhow::ensure!((1..=128).contains(&program), "GM instrument must be 1–128");
                options.programs.insert(part.into(), (program - 1) as u8);
            }
            "--tempo-scale" if command == "--midi" => options.tempo_scale = text()?.parse()?,
            "--play-seconds" if command == "--midi" => {
                let seconds: f64 = text()?.parse()?;
                anyhow::ensure!(
                    seconds.is_finite() && (0.1..=3600.0).contains(&seconds),
                    "Playback duration must be 0.1–3600 seconds"
                );
                play_seconds = Some(Duration::from_secs_f64(seconds));
            }
            _ => bail!("Unknown music option {option}"),
        }
    }
    if command == "--omr" {
        anyhow::ensure!(
            !play && play_seconds.is_none(),
            "Playback options apply to --midi"
        );
        std::fs::create_dir_all(&output)?;
        let hash = folder::hash_file(&input, &AtomicBool::new(false))?;
        let mut job = omr::RecognitionJob::start(input, hash, output.clone(), settings, || {})?;
        loop {
            for update in job.poll() {
                match update {
                    omr::RecognitionUpdate::Progress(message) => eprintln!("{message}"),
                    omr::RecognitionUpdate::Finished(result) => {
                        let artifacts = result.map_err(anyhow::Error::msg)?;
                        // The CLI uses only its explicit output directory, never the library DB.
                        println!("Recognized {} movement(s):", artifacts.movements.len());
                        for movement in artifacts.movements {
                            println!("{}", movement.display());
                        }
                        return Ok(());
                    }
                }
            }
            std::thread::sleep(Duration::from_millis(50));
        }
    }
    anyhow::ensure!(
        play || play_seconds.is_none(),
        "--play-seconds requires --play"
    );
    let composition = notation::load(&input)?;
    for warning in &composition.warnings {
        eprintln!("Warning: {warning}");
    }
    let bytes = notation::to_midi(&composition, &options)?;
    music::write_new(&output, &bytes)?;
    println!("MIDI exported: {}", output.display());
    if play {
        let mut playback = omr::Playback::start(&settings, &bytes)?;
        let start = Instant::now();
        while playback.poll()? {
            if play_seconds.is_some_and(|limit| start.elapsed() >= limit) {
                playback.stop();
                break;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        println!("Playback finished/stopped.");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn invalid_cli_options_do_not_create_a_destination() {
        let root = tempfile::tempdir().unwrap();
        let destination = root.path().join("out.mid");
        for tail in [
            vec!["--program", "P1=0"],
            vec!["--program", "P1=129"],
            vec!["--play-seconds", "NaN"],
            vec!["--play-seconds", "3"],
            vec!["--unknown", "x"],
        ] {
            let mut args = vec![
                OsString::from("missing.musicxml"),
                destination.clone().into_os_string(),
            ];
            args.extend(tail.into_iter().map(OsString::from));
            assert!(run("--midi", args).is_err());
            assert!(!destination.exists());
        }
    }
}
