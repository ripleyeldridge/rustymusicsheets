use super::*;
use std::os::unix::fs::symlink;

fn executable(root: &Path, body: &str) -> PathBuf {
    let path = root.join("fake helper with spaces");
    fs::write(&path, format!("#!/bin/sh\nset -eu\n{body}\n")).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
    path
}

#[test]
fn job_drop_kills_active_group_without_joining_or_channel_deadlock() {
    let root = tempfile::tempdir().unwrap();
    let marker = root.path().join("descendant");
    let helper = executable(
        root.path(),
        &format!(
            "sleep 60 &\nprintf '%s' \"$!\" > '{}'\nwait",
            marker.display()
        ),
    );
    let job = start_job(root.path(), helper);
    wait_for_file(&marker);
    let pid = fs::read_to_string(&marker).unwrap().parse::<u32>().unwrap();
    let start = Instant::now();
    drop(job);
    assert!(start.elapsed() < Duration::from_secs(1));
    let start = Instant::now();
    while process_running(pid) {
        assert!(start.elapsed() < Duration::from_secs(2));
        thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn settings_deserialization_preserves_defaults() {
    let settings: Settings = serde_json::from_str("{}").unwrap();
    assert_eq!(
        settings.audiveris_executable,
        Settings::default().audiveris_executable
    );
    assert_eq!(settings.fluidsynth_executable, "fluidsynth");
    let partial: Settings =
        serde_json::from_str(r#"{"audiveris_executable":"/my helper"}"#).unwrap();
    assert_eq!(partial.audiveris_executable, "/my helper");
    assert_eq!(partial.fluidsynth_executable, "fluidsynth");
}

fn finish(job: &mut RecognitionJob) -> std::result::Result<RecognitionArtifacts, String> {
    let start = Instant::now();
    loop {
        for update in job.poll() {
            if let RecognitionUpdate::Finished(result) = update {
                return result;
            }
        }
        assert!(start.elapsed() < Duration::from_secs(10), "job hung");
        thread::sleep(Duration::from_millis(10));
    }
}

fn source(root: &Path) -> (PathBuf, String) {
    let path = root.join("private source.PDF");
    fs::write(&path, b"synthetic test score").unwrap();
    let hash = crate::folder::hash_file(&path, &AtomicBool::new(false)).unwrap();
    (path, hash)
}

fn start_job(root: &Path, helper: PathBuf) -> RecognitionJob {
    let (source, hash) = source(root);
    RecognitionJob::start(
        source,
        hash,
        root.join("app-data"),
        Settings {
            audiveris_executable: helper.to_str().unwrap().into(),
            ..Settings::default()
        },
        || {},
    )
    .unwrap()
}

const EXPORT: &str = r#"
test "$1" = -batch
test "$2" = -transcribe
test "$3" = -export
test "$4" = -save
test "$5" = -output
out="$6"
test "$7" = --
test "${8##*.}" = pdf
mkdir -p "$out/book"
printf '<score-partwise/>' > "$out/book/one.musicxml"
printf 'compressed-fixture' > "$out/book/two.mxl"
"#;

#[test]
fn recognition_preserves_all_movements_privately_and_durably() {
    let root = tempfile::tempdir().unwrap();
    let helper = executable(root.path(), EXPORT);
    let mut job = start_job(root.path(), helper);
    let artifacts = finish(&mut job).unwrap();
    assert_eq!(artifacts.movements.len(), 2);
    assert_eq!(artifacts.source_hash.len(), 64);
    drop(job);
    for path in artifacts.movements {
        assert!(path.starts_with(root.path().join("app-data/omr")));
        assert!(path.is_file());
        let run = path.parent().unwrap().parent().unwrap().parent().unwrap();
        assert_eq!(
            fs::metadata(run).unwrap().permissions().mode() & 0o777,
            0o700
        );
        for name in ["engine-stdout.log", "engine-stderr.log"] {
            let log = run.join(name);
            assert!(log.is_file());
            assert_eq!(
                fs::metadata(log).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
    }
    assert!(!root.path().join("book").exists());
}

#[test]
fn recognition_reports_exit_failure_and_removes_run() {
    let root = tempfile::tempdir().unwrap();
    let helper = executable(
        root.path(),
        "printf 'synthetic recognition failure' >&2; exit 7",
    );
    let mut job = start_job(root.path(), helper);
    assert!(finish(&mut job)
        .unwrap_err()
        .contains("synthetic recognition failure"));
    for entry in WalkDir::new(root.path().join("app-data")) {
        assert!(!entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with("run-"));
    }
}

#[test]
fn stale_source_is_rejected_before_execution() {
    let root = tempfile::tempdir().unwrap();
    let (source, hash) = source(root.path());
    fs::write(&source, b"changed").unwrap();
    let mut job = RecognitionJob::start(
        source,
        hash,
        root.path().join("app-data"),
        Settings::default(),
        || {},
    )
    .unwrap();
    assert!(finish(&mut job).unwrap_err().contains("changed"));
    assert!(!root.path().join("app-data").exists());
    assert!(validate_hash("../escape").is_err());
}

#[test]
fn source_modified_during_recognition_is_rejected() {
    let root = tempfile::tempdir().unwrap();
    let helper = executable(
        root.path(),
        &format!(
            "{EXPORT}\nprintf changed > '{}'",
            root.path().join("private source.PDF").display()
        ),
    );
    let mut job = start_job(root.path(), helper);
    assert!(finish(&mut job).unwrap_err().contains("changed"));
}

#[test]
fn output_rejects_symlinks_and_oversize() {
    let root = tempfile::tempdir().unwrap();
    symlink("/etc/passwd", root.path().join("unsafe.xml")).unwrap();
    assert!(inspect_tree(root.path())
        .unwrap_err()
        .to_string()
        .contains("symlink"));
    fs::remove_file(root.path().join("unsafe.xml")).unwrap();
    File::create(root.path().join("large.mxl"))
        .unwrap()
        .set_len(FILE_LIMIT + 1)
        .unwrap();
    assert!(inspect_tree(root.path())
        .unwrap_err()
        .to_string()
        .contains("size limit"));
}

fn wait_for_file(path: &Path) {
    let start = Instant::now();
    while !path.exists() {
        assert!(
            start.elapsed() < Duration::from_secs(5),
            "helper did not start"
        );
        thread::sleep(Duration::from_millis(10));
    }
}

fn process_running(pid: u32) -> bool {
    fs::read_to_string(format!("/proc/{pid}/stat"))
        .ok()
        .and_then(|s| {
            s.rsplit_once(") ")
                .map(|(_, state)| !state.starts_with('Z'))
        })
        .unwrap_or(false)
}

#[test]
fn cancellation_kills_child_and_descendants() {
    let root = tempfile::tempdir().unwrap();
    let marker = root.path().join("descendant");
    let helper = executable(
        root.path(),
        &format!(
            "sleep 60 &\nprintf '%s' \"$!\" > '{}'\nwait",
            marker.display()
        ),
    );
    let mut job = start_job(root.path(), helper);
    wait_for_file(&marker);
    let pid = fs::read_to_string(&marker).unwrap().parse::<u32>().unwrap();
    assert!(process_running(pid));
    job.cancel();
    assert!(finish(&mut job).unwrap_err().contains("cancelled"));
    assert!(!process_running(pid));
}

#[test]
fn timeout_and_log_limits_terminate_helpers() {
    let mut timeout =
        ManagedProcess::spawn(Command::new("/bin/sleep").arg("60"), Duration::ZERO).unwrap();
    assert!(timeout
        .poll()
        .unwrap_err()
        .to_string()
        .contains("timed out"));
    assert!(timeout.child.is_none());
    let mut logs =
        ManagedProcess::spawn(Command::new("/bin/sleep").arg("60"), Duration::from_secs(5))
            .unwrap();
    // Log readers are read-only; grow the synthetic child's output through its
    // private backing file to avoid racing a shell's output rate.
    File::options()
        .write(true)
        .open(logs._logs.path().join("stderr"))
        .unwrap()
        .set_len(LOG_LIMIT + 1)
        .unwrap();
    assert!(logs.poll().unwrap_err().to_string().contains("2 MiB"));
    assert!(logs.child.is_none());
}

fn playback_settings(root: &Path, body: &str) -> Settings {
    let soundfont = root.join("test soundfont.sf2");
    fs::write(&soundfont, b"RIFF\x04\x00\x00\x00sfbk").unwrap();
    Settings {
        fluidsynth_executable: executable(root, body).to_str().unwrap().into(),
        soundfont: soundfont.to_str().unwrap().into(),
        ..Settings::default()
    }
}

const MIDI: &[u8] = b"MThd\x00\x00\x00\x06\x00\x00\x00\x01\x00\x60";

fn playback_finish(playback: &mut Playback) -> Result<()> {
    let start = Instant::now();
    while playback.poll()? {
        assert!(start.elapsed() < Duration::from_secs(5));
        thread::sleep(Duration::from_millis(10));
    }
    Ok(())
}

#[test]
fn playback_owns_midi_checks_arguments_and_cleans_up() {
    let root = tempfile::tempdir().unwrap();
    let settings = playback_settings(
        root.path(),
        "test \"$1\" = -ni; test -f \"$2\"; test -f \"$3\"; sleep 0.1",
    );
    let mut playback = Playback::start(&settings, MIDI).unwrap();
    let midi_path = playback
        ._private_files
        .as_ref()
        .unwrap()
        .path()
        .join("playback.mid");
    assert_eq!(fs::read(&midi_path).unwrap(), MIDI);
    playback_finish(&mut playback).unwrap();
    assert!(!playback.poll().unwrap());
    drop(playback);
    assert!(!midi_path.exists());
}

#[test]
fn playback_nonzero_and_successful_exit_audio_failures_are_errors() {
    let root = tempfile::tempdir().unwrap();
    let settings = playback_settings(root.path(), "printf 'cannot initialize synth' >&2; exit 2");
    let mut playback = Playback::start(&settings, MIDI).unwrap();
    assert!(playback_finish(&mut playback)
        .unwrap_err()
        .to_string()
        .contains("cannot initialize"));
    let settings = playback_settings(
        root.path(),
        "printf 'fluidsynth: error: Failed to create the audio driver' >&2; sleep 60",
    );
    let mut playback = Playback::start(&settings, MIDI).unwrap();
    assert!(playback_finish(&mut playback)
        .unwrap_err()
        .to_string()
        .contains("audio"));
    assert!(playback.process.child.is_none());
}

#[test]
fn playback_stop_and_drop_kill_process_and_remove_files() {
    let root = tempfile::tempdir().unwrap();
    let settings = playback_settings(root.path(), "sleep 60");
    let mut playback = Playback::start(&settings, MIDI).unwrap();
    let pid = playback.process.group;
    playback.stop();
    assert!(!process_running(pid));
    assert!(!playback.poll().unwrap());
    let playback = Playback::start(&settings, MIDI).unwrap();
    let pid = playback.process.group;
    let midi = playback
        ._private_files
        .as_ref()
        .unwrap()
        .path()
        .join("playback.mid");
    drop(playback);
    assert!(!process_running(pid));
    assert!(!midi.exists());
}

#[test]
fn playback_rejects_bad_soundfont_and_missing_executable() {
    let root = tempfile::tempdir().unwrap();
    let mut settings = playback_settings(root.path(), "exit 0");
    settings.fluidsynth_executable = root.path().join("missing").to_str().unwrap().into();
    assert!(Playback::start(&settings, MIDI).is_err());
    fs::write(&settings.soundfont, b"not a soundfont").unwrap();
    assert!(Playback::start(&settings, MIDI).is_err());
    assert!(Playback::start(&settings, b"bad MIDI").is_err());
}
