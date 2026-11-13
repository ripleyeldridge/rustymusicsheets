use super::*;
use crate::{import, model::Paths, storage::Library};
use std::{
    io::{self, Cursor},
    sync::Arc,
};

struct FakeDocuments {
    metadata: Metadata,
    bytes: Vec<u8>,
    fail_open: bool,
    fail_read: bool,
    cancel_on_read: Option<Arc<AtomicBool>>,
    closed: Arc<AtomicBool>,
}

struct FakeStream {
    bytes: Cursor<Vec<u8>>,
    fail: bool,
    cancel: Option<Arc<AtomicBool>>,
    closed: Arc<AtomicBool>,
}

impl Read for FakeStream {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        if self.fail {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "read grant revoked",
            ));
        }
        if let Some(cancel) = &self.cancel {
            cancel.store(true, Ordering::Relaxed);
        }
        self.bytes.read(buffer)
    }
}

impl Drop for FakeStream {
    fn drop(&mut self) {
        self.closed.store(true, Ordering::Relaxed);
    }
}

impl DocumentAccess for FakeDocuments {
    fn metadata(&self, _source: &DocumentRef) -> Result<Metadata> {
        Ok(self.metadata.clone())
    }
    fn open(&self, _source: &DocumentRef) -> Result<Box<dyn Read + Send>> {
        if self.fail_open {
            bail!("read permission denied");
        }
        Ok(Box::new(FakeStream {
            bytes: Cursor::new(self.bytes.clone()),
            fail: self.fail_read,
            cancel: self.cancel_on_read.clone(),
            closed: self.closed.clone(),
        }))
    }
}

fn fake() -> FakeDocuments {
    FakeDocuments {
        metadata: Metadata {
            display_name: Some("Cello.png".into()),
            mime_type: Some("image/png".into()),
            size: None,
        },
        bytes: b"example".to_vec(),
        fail_open: false,
        fail_read: false,
        cancel_on_read: None,
        closed: Arc::new(AtomicBool::new(false)),
    }
}

fn reference() -> DocumentRef {
    DocumentRef::Content(
        ContentUri::parse("content://test.documents/document/music%3ACello.png").unwrap(),
    )
}

#[test]
fn uri_encoding_is_preserved_and_never_becomes_a_path() {
    let raw =
        "content://org.example.documents/document/primary%3AScores%2F%C3%89tude.pdf?view=read";
    let content = ContentUri::parse(raw).unwrap();
    assert_eq!(content.as_str(), raw);
    let reference = DocumentRef::Content(content);
    assert!(reference.as_path().is_none());
    let encoded = serde_json::to_string(&reference).unwrap();
    assert_eq!(
        serde_json::from_str::<DocumentRef>(&encoded).unwrap(),
        reference
    );
}

#[test]
fn invalid_and_wrong_scheme_uris_are_rejected_even_on_deserialization() {
    for invalid in [
        "",
        "file:///tmp/Score.pdf",
        "/tmp/Score.pdf",
        "https://example.com/score.pdf",
        "content:///score.pdf",
        "content://",
        "content://provider/a b",
        "content://provider/a\nb",
        "content://provider/a#fragment",
        "content://provider:80/a",
    ] {
        assert!(ContentUri::parse(invalid).is_err(), "{invalid:?}");
        let json = serde_json::json!({"kind":"Content","value":invalid});
        assert!(serde_json::from_value::<DocumentRef>(json).is_err());
    }
}

#[test]
fn provider_names_and_mime_types_are_checked_before_writing() {
    for invalid in [
        "../escape.png",
        "dir/score.png",
        r"dir\score.png",
        ".",
        "..",
        "C:score.png",
        "",
        "score\0.png",
    ] {
        let metadata = Metadata {
            display_name: Some(invalid.into()),
            mime_type: Some("image/png".into()),
            ..Default::default()
        };
        assert!(metadata.import_name().is_err(), "{invalid:?}");
    }
    assert!(Metadata {
        display_name: Some("Cello.pdf".into()),
        mime_type: Some("image/png".into()),
        ..Default::default()
    }
    .import_name()
    .is_err());
    assert_eq!(
        Metadata {
            mime_type: Some("image/jpeg".into()),
            ..Default::default()
        }
        .import_name()
        .unwrap(),
        "Imported score.jpg"
    );
    assert_eq!(
        Metadata {
            display_name: Some("Étude".into()),
            mime_type: Some("image/png; charset=binary".into()),
            ..Default::default()
        }
        .import_name()
        .unwrap(),
        "Étude.png"
    );
}

#[test]
fn staged_documents_are_private_and_disappear_on_drop() {
    let cache = tempfile::tempdir().unwrap();
    let access = fake();
    let staged = stage(&reference(), &access, cache.path(), &AtomicBool::new(false)).unwrap();
    assert_eq!(std::fs::read(&staged.path).unwrap(), access.bytes);
    assert!(staged.path.starts_with(cache.path()));
    let path = staged.path.clone();
    assert!(access.closed.load(Ordering::Relaxed));
    drop(staged);
    assert!(!path.exists());
    assert_eq!(std::fs::read_dir(cache.path()).unwrap().count(), 0);
}

#[test]
fn oversized_unknown_length_streams_and_inaccurate_sizes_leave_no_partial_file() {
    let cache = tempfile::tempdir().unwrap();
    let mut access = fake();
    assert!(stage_with_limit(
        &reference(),
        &access,
        cache.path(),
        &AtomicBool::new(false),
        3
    )
    .is_err());
    assert!(access.closed.load(Ordering::Relaxed));
    access.metadata.size = Some(999);
    access.fail_open = true;
    let error = stage_with_limit(
        &reference(),
        &access,
        cache.path(),
        &AtomicBool::new(false),
        10,
    )
    .err()
    .unwrap();
    assert!(error.to_string().contains("size limit"));
    access.fail_open = false;
    access.metadata.size = Some(8);
    assert!(stage(&reference(), &access, cache.path(), &AtomicBool::new(false)).is_err());
    assert_eq!(std::fs::read_dir(cache.path()).unwrap().count(), 0);
}

#[test]
fn denied_revoked_and_cancelled_reads_close_and_clean_up() {
    let cache = tempfile::tempdir().unwrap();
    let mut access = fake();
    access.fail_open = true;
    assert!(stage(&reference(), &access, cache.path(), &AtomicBool::new(false)).is_err());
    access.fail_open = false;
    access.fail_read = true;
    assert!(stage(&reference(), &access, cache.path(), &AtomicBool::new(false)).is_err());
    assert!(access.closed.load(Ordering::Relaxed));
    access.fail_read = false;
    let cancel = Arc::new(AtomicBool::new(false));
    access.cancel_on_read = Some(cancel.clone());
    assert!(stage(&reference(), &access, cache.path(), &cancel).is_err());
    assert!(stage(&reference(), &access, cache.path(), &AtomicBool::new(true)).is_err());
    assert_eq!(std::fs::read_dir(cache.path()).unwrap().count(), 0);
}

#[test]
fn uri_image_import_uses_existing_deduplication_and_keeps_only_managed_copy() {
    let root = tempfile::tempdir().unwrap();
    let paths = Paths::app_private(&root.path().join("files"), &root.path().join("cache")).unwrap();
    let db = Library::open(&paths).unwrap();
    let mut access = fake();
    let image = image::RgbImage::from_pixel(40, 60, image::Rgb([255, 255, 255]));
    let mut encoded = Cursor::new(Vec::new());
    image
        .write_to(&mut encoded, image::ImageFormat::Png)
        .unwrap();
    access.bytes = encoded.into_inner();
    access.metadata.size = Some(access.bytes.len() as u64);
    let cancel = AtomicBool::new(false);
    assert_eq!(
        import::import_document(&reference(), &access, &paths, &db, &cancel).unwrap(),
        import::Outcome::Imported
    );
    assert_eq!(
        import::import_document(&reference(), &access, &paths, &db, &cancel).unwrap(),
        import::Outcome::Duplicate
    );
    let score = db.scores().unwrap().remove(0);
    assert_eq!(score.title, "Cello");
    assert_eq!(score.pages, 1);
    assert_eq!(
        std::fs::read(db.pdf_path(&paths, &score.hash).unwrap()).unwrap(),
        access.bytes
    );
    assert_eq!(std::fs::read_dir(&paths.cache).unwrap().count(), 0);
    assert!(LocalDocuments.open(&reference()).is_err());
}

#[test]
fn app_private_paths_keep_metadata_out_of_disposable_cache() {
    let root = tempfile::tempdir().unwrap();
    let files = root.path().join("files");
    let cache = root.path().join("cache");
    let paths = Paths::app_private(&files, &cache).unwrap();
    assert!(paths.data.starts_with(&files));
    assert!(paths.config.starts_with(&files));
    assert!(paths.cache.starts_with(&cache));
    assert!(paths.score_dir().is_dir());
    assert!(Paths::app_private(Path::new("relative"), &cache).is_err());
}
