use super::{FilePicker, Request, SelectionKind};
use anyhow::{bail, Context, Result};
use std::{
    ffi::OsString,
    os::unix::ffi::OsStringExt,
    path::PathBuf,
    process::{Command, Output},
};

pub(super) struct Backend;

fn command(request: &Request) -> Command {
    let filename = request
        .filename
        .as_ref()
        .map(|name| request.directory.join(name))
        .unwrap_or_else(|| request.directory.clone());
    let mut filename = filename.into_os_string();
    if request.filename.is_none() {
        filename.push(std::path::MAIN_SEPARATOR_STR);
    }
    let mut command = Command::new("zenity");
    command
        .current_dir(&request.directory)
        .arg("--file-selection")
        .arg(format!(
            "--title=RustyMusicSheets — {}",
            request.kind.title()
        ))
        .arg("--filename")
        .arg(filename);
    if request.kind == SelectionKind::Folder {
        command.arg("--directory");
    } else if !request.kind.extensions().is_empty() {
        let filters = request
            .kind
            .extensions()
            .iter()
            .map(|extension| format!("*.{extension} *.{}", extension.to_ascii_uppercase()))
            .collect::<Vec<_>>()
            .join(" ");
        command.arg(format!(
            "--file-filter={} | {filters}",
            request.kind.title()
        ));
    }
    if request.kind.is_save() {
        command.arg("--save");
    }
    command
}

fn parse_output(output: Output) -> Result<Option<PathBuf>> {
    if output.status.code() == Some(1) {
        return Ok(None);
    }
    if !output.status.success() {
        bail!(
            "File picker failed ({}): {}. Paste a path instead.",
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    let mut bytes = output.stdout;
    if bytes.last() == Some(&b'\n') {
        bytes.pop();
    }
    if bytes.is_empty() {
        bail!("File picker returned an empty path; paste a path instead");
    }
    Ok(Some(PathBuf::from(OsString::from_vec(bytes))))
}

impl FilePicker for Backend {
    fn select(&self, request: &Request) -> Result<Option<PathBuf>> {
        let output = command(request)
            .output()
            .context("File picker unavailable; install Zenity or paste a path instead")?;
        parse_output(output)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::{ffi::OsStrExt, process::ExitStatusExt};

    #[test]
    fn typed_open_and_save_flags_keep_unicode_names_and_correct_filters() {
        let root = tempfile::tempdir().unwrap();
        for kind in [
            SelectionKind::Executable,
            SelectionKind::SoundFont,
            SelectionKind::MusicXml,
            SelectionKind::SaveMxl,
            SelectionKind::SaveMusicXml,
            SelectionKind::SaveMidi,
        ] {
            let request = Request {
                kind,
                directory: root.path().to_owned(),
                filename: Some("Été with spaces.mid".into()),
            };
            let command = command(&request);
            let args: Vec<_> = command.get_args().collect();
            assert_eq!(args.iter().any(|arg| *arg == "--save"), kind.is_save());
            assert!(!args.iter().any(|arg| *arg == "--directory"));
            assert!(args
                .iter()
                .any(|arg| *arg == root.path().join("Été with spaces.mid").as_os_str()));
            for ext in kind.extensions() {
                assert!(args
                    .iter()
                    .any(|arg| arg.to_string_lossy().contains(&format!("*.{ext}"))));
            }
            if kind == SelectionKind::Executable {
                assert!(!args
                    .iter()
                    .any(|arg| arg.to_string_lossy().starts_with("--file-filter")));
            }
        }
    }

    #[test]
    fn command_keeps_directory_and_score_filters() {
        let root = tempfile::tempdir().unwrap();
        for kind in [SelectionKind::Score, SelectionKind::Folder] {
            let request = Request {
                kind,
                directory: root.path().to_owned(),
                filename: None,
            };
            let command = command(&request);
            assert_eq!(command.get_current_dir(), Some(root.path()));
            let args: Vec<_> = command.get_args().map(|a| a.to_string_lossy()).collect();
            assert_eq!(
                args.iter().any(|a| a == "--directory"),
                kind == SelectionKind::Folder
            );
            if kind == SelectionKind::Score {
                for extension in super::super::SCORE_EXTENSIONS {
                    assert!(args.iter().any(|a| a.contains(&format!("*.{extension}"))));
                }
            }
        }
    }

    fn output(code: i32, stdout: &[u8]) -> Output {
        Output {
            status: std::process::ExitStatus::from_raw(code << 8),
            stdout: stdout.to_vec(),
            stderr: b"dialog error".to_vec(),
        }
    }

    #[test]
    fn cancellation_is_not_a_failure_and_invalid_results_are_errors() {
        assert!(parse_output(output(1, b"")).unwrap().is_none());
        assert!(parse_output(output(2, b""))
            .unwrap_err()
            .to_string()
            .contains("dialog error"));
        assert!(parse_output(output(0, b"")).is_err());
    }

    #[test]
    fn paths_preserve_non_utf8_and_embedded_newlines() {
        let bytes = b"/tmp/Etude \xff\npart.pdf\n";
        let selected = parse_output(output(0, bytes)).unwrap().unwrap();
        assert_eq!(selected.as_os_str().as_bytes(), &bytes[..bytes.len() - 1]);
    }
}
