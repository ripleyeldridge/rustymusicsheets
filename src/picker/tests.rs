use super::*;
use std::cell::RefCell;

#[test]
fn save_requests_keep_suggested_names_and_add_only_the_correct_extension() {
    let root = tempfile::tempdir().unwrap();
    for (kind, extension) in [
        (SelectionKind::SaveMxl, "mxl"),
        (SelectionKind::SaveMusicXml, "musicxml"),
        (SelectionKind::SaveMidi, "mid"),
    ] {
        let selected = root.path().join("Éléphant duet");
        let fake = FakePicker {
            seen: RefCell::new(None),
            selected: Some(selected.clone()),
            fail: false,
        };
        let preferred = root.path().join(format!("new-export.{extension}"));
        let result = select_with(&fake, kind, &preferred, root.path())
            .unwrap()
            .unwrap();
        assert_eq!(result, selected.with_extension(extension));
        let request = fake.seen.borrow();
        let request = request.as_ref().unwrap();
        assert_eq!(request.filename.as_deref(), preferred.file_name());
        assert_eq!(request.directory, root.path().canonicalize().unwrap());
        assert!(
            !result.exists(),
            "Selecting a filename must not write an export"
        );
        assert!(normalize_save_path(kind, root.path().join("wrong.pdf")).is_err());
    }
    for kind in [
        SelectionKind::Executable,
        SelectionKind::SoundFont,
        SelectionKind::MusicXml,
    ] {
        let fake = FakePicker {
            seen: RefCell::new(None),
            selected: None,
            fail: false,
        };
        assert!(select_with(&fake, kind, root.path(), root.path())
            .unwrap()
            .is_none());
        assert_eq!(fake.seen.borrow().as_ref().unwrap().kind, kind);
    }
}

struct FakePicker {
    seen: RefCell<Option<Request>>,
    selected: Option<PathBuf>,
    fail: bool,
}

impl FilePicker for FakePicker {
    fn select(&self, request: &Request) -> Result<Option<PathBuf>> {
        *self.seen.borrow_mut() = Some(request.clone());
        if self.fail {
            anyhow::bail!("test dialog failed");
        }
        Ok(self.selected.clone())
    }
}

#[test]
fn missing_directory_falls_back_to_home_without_creating_it() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().join("Home");
    std::fs::create_dir(&home).unwrap();
    let missing = temp.path().join("Missing library");
    assert_eq!(
        initial_directory(&missing, &home),
        home.canonicalize().unwrap()
    );
    assert!(!missing.exists());
}

#[test]
fn existing_folder_or_score_uses_its_own_directory() {
    let temp = tempfile::tempdir().unwrap();
    let folder = temp.path().join("Études with spaces");
    std::fs::create_dir(&folder).unwrap();
    let expected = folder.canonicalize().unwrap();
    assert_eq!(initial_directory(&folder, temp.path()), expected);
    let file = folder.join("Score.pdf");
    std::fs::write(&file, b"test").unwrap();
    assert_eq!(initial_directory(&file, temp.path()), expected);
    let fallback = initial_directory(
        &temp.path().join("missing"),
        &temp.path().join("missing home"),
    );
    assert!(fallback.is_dir());
}

#[test]
fn typed_request_keeps_path_kind_cancellation_and_errors_separate() {
    let root = tempfile::tempdir().unwrap();
    let selected = root.path().join("Éléphant cello.png");
    let mut fake = FakePicker {
        seen: RefCell::new(None),
        selected: Some(selected.clone()),
        fail: false,
    };
    let missing = root.path().join("Missing");
    for kind in [SelectionKind::Score, SelectionKind::Folder] {
        assert_eq!(
            select_with(&fake, kind, &missing, root.path()).unwrap(),
            Some(selected.clone())
        );
        assert_eq!(fake.seen.borrow().as_ref().unwrap().kind, kind);
        assert_eq!(
            fake.seen.borrow().as_ref().unwrap().directory,
            root.path().canonicalize().unwrap()
        );
    }
    fake.selected = None;
    assert!(
        select_with(&fake, SelectionKind::Score, &missing, root.path())
            .unwrap()
            .is_none()
    );
    fake.fail = true;
    assert!(select_with(&fake, SelectionKind::Score, &missing, root.path()).is_err());
}

#[test]
fn unsupported_platform_returns_actionable_error() {
    let root = tempfile::tempdir().unwrap();
    let error = select_with(
        &unsupported::Backend,
        SelectionKind::Score,
        root.path(),
        root.path(),
    )
    .unwrap_err();
    assert!(error.to_string().contains("paste a path"));
}
