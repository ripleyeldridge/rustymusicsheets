//! Sidecars are independent of source files. Rename is the write commit point.
//! Locks coordinate participating writers and deliberately fail closed after crashes.
use super::model::{valid_hash, PageInk, MAX_BYTES};
use anyhow::{bail, ensure, Context, Result};
use sha2::{Digest, Sha256};
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use tempfile::NamedTempFile;

#[derive(Debug)]
pub struct Loaded {
    pub ink: PageInk,
    pub token: Option<String>,
    pub error: Option<String>,
}

#[derive(Debug)]
pub struct Saved {
    pub token: String,
    pub durability_warning: Option<String>,
}

#[derive(Debug)]
pub struct Recovered {
    pub saved: Saved,
    pub backup: PathBuf,
}

struct Raw {
    bytes: Option<Vec<u8>>,
    token: String,
}

fn page_path(data: &Path, hash: &str, page: u32, pages: u32) -> Result<PathBuf> {
    ensure!(valid_hash(hash), "invalid score hash");
    ensure!(page < pages, "annotation page out of range");
    Ok(data
        .join("annotations")
        .join(hash)
        .join(format!("{page}.ink-v1.json")))
}

// Oversized files have no CAS token and require manual preservation/removal.
// Read at most one byte beyond the ceiling, including concurrently growing files.
fn read_raw(path: &Path) -> Result<Option<Raw>> {
    match fs::symlink_metadata(path) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e.into()),
        Ok(meta) => ensure!(meta.file_type().is_file(), "sidecar is not a regular file"),
    }
    let file = File::open(path).context("open annotation sidecar")?;
    let mut bytes = Vec::new();
    file.take((MAX_BYTES + 1) as u64)
        .read_to_end(&mut bytes)
        .context("read annotation sidecar")?;
    if bytes.len() > MAX_BYTES {
        return Ok(Some(Raw {
            bytes: None,
            token: String::new(),
        }));
    }
    let token = format!("{:x}", Sha256::digest(&bytes));
    Ok(Some(Raw {
        bytes: Some(bytes),
        token,
    }))
}

fn decode(raw: &Raw, hash: &str, page: u32, pages: u32) -> Result<PageInk> {
    let bytes = raw.bytes.as_deref().context(
        "annotation exceeds 8 MiB; manually preserve and move the sidecar before reloading",
    )?;
    let ink: PageInk = serde_json::from_slice(bytes).context("invalid annotation JSON")?;
    ink.validate(pages)?;
    ensure!(
        ink.score_hash == hash && ink.page == page,
        "annotation identity mismatch"
    );
    Ok(ink)
}

pub fn load(data: &Path, hash: &str, page: u32, pages: u32) -> Result<Loaded> {
    let path = page_path(data, hash, page, pages)?;
    let mut loaded = Loaded {
        ink: PageInk::empty(hash, page),
        token: None,
        error: None,
    };
    if let Some(raw) = read_raw(&path)? {
        loaded.token = raw.bytes.as_ref().map(|_| raw.token.clone());
        match decode(&raw, hash, page, pages) {
            Ok(ink) => loaded.ink = ink,
            Err(error) => loaded.error = Some(format!("{error:#}")),
        }
    }
    Ok(loaded)
}

struct PageLock {
    path: PathBuf,
    _file: File,
}

impl PageLock {
    fn acquire(path: &Path) -> Result<Self> {
        let path = path.with_extension("json.lock");
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let file = options.open(&path).with_context(|| {
            format!(
                "annotation write lock unavailable (another writer or stale lock): {}",
                path.display()
            )
        })?;
        Ok(Self { path, _file: file })
    }
}

impl Drop for PageLock {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

fn prepare_directory(path: &Path) -> Result<()> {
    let mut builder = fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder.create(path).context("create annotation directory")
}

fn check_expected(raw: Option<&Raw>, expected: Option<&str>) -> Result<()> {
    ensure!(
        raw.is_none_or(|r| r.bytes.is_some()),
        "oversized annotation is read-only; manually preserve and move it before reloading"
    );
    ensure!(
        raw.map(|r| r.token.as_str()) == expected,
        "annotation conflict: sidecar changed since load"
    );
    Ok(())
}

// Serialize through a capped writer instead of building an unbounded JSON Vec.
struct Payload(Vec<u8>);
impl Write for Payload {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        if self.0.len().saturating_add(bytes.len()) > MAX_BYTES {
            return Err(std::io::Error::other("annotation exceeds 8 MiB"));
        }
        self.0.extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

fn payload(ink: &PageInk, pages: u32) -> Result<Vec<u8>> {
    ink.validate(pages)?;
    let mut out = Payload(Vec::new());
    serde_json::to_writer(&mut out, ink).context("serialize annotation")?;
    Ok(out.0)
}

fn replacement(path: &Path, bytes: &[u8]) -> Result<Saved> {
    replacement_with_sync(path, bytes, sync_directory)
}

fn replacement_with_sync(
    path: &Path,
    bytes: &[u8],
    sync: impl FnOnce(&Path) -> Result<()>,
) -> Result<Saved> {
    let parent = path.parent().context("missing annotation directory")?;
    let mut temp = NamedTempFile::new_in(parent).context("create private annotation tempfile")?;
    temp.write_all(bytes).context("write annotation tempfile")?;
    temp.as_file()
        .sync_all()
        .context("sync annotation tempfile")?;
    temp.persist(path).context("replace annotation sidecar")?;
    // No fallible operation after rename may masquerade as a rollback.
    let durability_warning = sync(parent).err().map(|e| format!("{e:#}"));
    Ok(Saved {
        token: format!("{:x}", Sha256::digest(bytes)),
        durability_warning,
    })
}

fn sync_directory(path: &Path) -> Result<()> {
    // Include the links for newly-created hash and annotations directories.
    for directory in path.ancestors().take(3) {
        File::open(directory)
            .and_then(|f| f.sync_all())
            .with_context(|| format!("directory sync failed: {}", directory.display()))?;
    }
    Ok(())
}

pub fn save(data: &Path, ink: &PageInk, pages: u32, expected: Option<&str>) -> Result<Saved> {
    let bytes = payload(ink, pages)?;
    let path = page_path(data, &ink.score_hash, ink.page, pages)?;
    prepare_directory(path.parent().context("missing annotation directory")?)?;
    let _lock = PageLock::acquire(&path)?;
    let current = read_raw(&path)?;
    check_expected(current.as_ref(), expected)?;
    if let Some(raw) = current {
        decode(&raw, &ink.score_hash, ink.page, pages)
            .context("rejected sidecar is read-only; explicit recovery required")?;
    }
    replacement(&path, &bytes)
}

pub fn recover(
    data: &Path,
    empty: &PageInk,
    pages: u32,
    expected: Option<&str>,
) -> Result<Recovered> {
    ensure!(empty.strokes.is_empty(), "recovery requires an empty page");
    let bytes = payload(empty, pages)?;
    let path = page_path(data, &empty.score_hash, empty.page, pages)?;
    let parent = path.parent().context("missing annotation directory")?;
    // Recovery does not create an absent page or replace healthy annotations.
    let _lock = PageLock::acquire(&path)?;
    let current = read_raw(&path)?;
    check_expected(current.as_ref(), expected)?;
    let raw = current.context("no rejected annotation to recover")?;
    ensure!(
        decode(&raw, &empty.score_hash, empty.page, pages).is_err(),
        "annotation is valid; recovery refused"
    );
    let mut backup = NamedTempFile::new_in(parent).context("create private recovery backup")?;
    let mut source = File::open(&path)?;
    let mut hasher = Sha256::new();
    let mut buf = [0u8; 16 * 1024];
    let mut total = 0usize;
    loop {
        let remaining = (MAX_BYTES + 1 - total).min(buf.len());
        let count = source.read(&mut buf[..remaining])?;
        if count == 0 {
            break;
        }
        total += count;
        ensure!(
            total <= MAX_BYTES,
            "annotation grew beyond limit during backup"
        );
        hasher.update(&buf[..count]);
        backup.write_all(&buf[..count])?;
    }
    ensure!(
        format!("{:x}", hasher.finalize()) == raw.token,
        "annotation changed during backup"
    );
    backup
        .as_file()
        .sync_all()
        .context("sync recovery backup")?;
    // Persist with an unpredictable name in the same private directory.
    let backup_path = backup.path().with_extension("ink-rejected.bak");
    backup
        .persist_noclobber(&backup_path)
        .context("preserve recovery backup")?;
    if let Err(error) = sync_directory(parent) {
        bail!(
            "backup preserved at {}, original not replaced: {error:#}",
            backup_path.display()
        );
    }
    let saved = replacement(&path, &bytes).with_context(|| {
        format!(
            "recovery failed; backup preserved at {}",
            backup_path.display()
        )
    })?;
    Ok(Recovered {
        saved,
        backup: backup_path,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::annotations::model::{InkTool, Point, Stroke, MAX_POINTS, MAX_STROKES};
    use std::sync::{Arc, Barrier};
    const HASH: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

    fn path(data: &Path) -> PathBuf {
        page_path(data, HASH, 0, 1).unwrap()
    }
    fn write_fixture(data: &Path, bytes: &[u8]) {
        prepare_directory(path(data).parent().unwrap()).unwrap();
        fs::write(path(data), bytes).unwrap();
    }
    #[test]
    fn save_reload_clear_and_source_bytes_unchanged() {
        let dir = tempfile::tempdir().unwrap();
        let data = dir.path();
        let source = data.join("generated-source.pdf");
        let original = b"%PDF-1.4 generated fixture only";
        fs::write(&source, original).unwrap();
        let loaded = load(data, HASH, 0, 1).unwrap();
        assert!(loaded.error.is_none() && loaded.token.is_none());
        assert!(!data.join("annotations").exists());
        let mut ink = loaded.ink;
        ink.strokes.push(Stroke {
            tool: InkTool::Highlighter,
            color: [1, 2, 3, 80],
            width: 0.02,
            points: vec![Point { x: 0.5, y: 0.5 }],
        });
        let saved = save(data, &ink, 1, None).unwrap();
        assert!(saved.durability_warning.is_none());
        let loaded = load(data, HASH, 0, 1).unwrap();
        assert_eq!(loaded.ink, ink);
        assert_eq!(loaded.token.as_deref(), Some(saved.token.as_str()));
        ink.strokes.clear();
        save(data, &ink, 1, Some(&saved.token)).unwrap();
        assert!(load(data, HASH, 0, 1).unwrap().ink.strokes.is_empty());
        assert_eq!(fs::read(source).unwrap(), original);
    }
    #[test]
    fn rejected_files_are_read_only_and_recovery_preserves_exact_bytes() {
        let dir = tempfile::tempdir().unwrap();
        let data = dir.path();
        let empty = PageInk::empty(HASH, 0);
        let mut wrong_hash = empty.clone();
        wrong_hash.score_hash = "b".repeat(64);
        let mut wrong_page = empty.clone();
        wrong_page.page = 1;
        let mut newer = empty.clone();
        newer.version = 2;
        let fixtures = [
            b"{broken".to_vec(),
            serde_json::to_vec(&wrong_hash).unwrap(),
            serde_json::to_vec(&wrong_page).unwrap(),
            serde_json::to_vec(&newer).unwrap(),
            format!(r#"{{"version":1,"score_hash":"{HASH}","page":0,"strokes":[],"future":1}}"#)
                .into_bytes(),
        ];
        for bytes in fixtures {
            write_fixture(data, &bytes);
            let loaded = load(data, HASH, 0, 1).unwrap();
            assert!(loaded.error.is_some());
            assert_eq!(loaded.ink, empty);
            assert!(loaded.token.is_some());
            assert!(save(data, &empty, 1, loaded.token.as_deref()).is_err());
            assert_eq!(fs::read(path(data)).unwrap(), bytes);
            assert!(recover(data, &empty, 1, None).is_err());
            let recovered = recover(data, &empty, 1, loaded.token.as_deref()).unwrap();
            assert_eq!(fs::read(&recovered.backup).unwrap(), bytes);
            let loaded = load(data, HASH, 0, 1).unwrap();
            assert!(loaded.error.is_none());
            assert_eq!(loaded.token, Some(recovered.saved.token));
        }
        let loaded = load(data, HASH, 0, 1).unwrap();
        assert!(recover(data, &empty, 1, loaded.token.as_deref()).is_err());
    }
    #[test]
    fn oversized_files_have_no_token_and_cannot_be_replaced_or_recovered() {
        let dir = tempfile::tempdir().unwrap();
        let data = dir.path();
        let bytes = vec![b' '; MAX_BYTES + 1];
        write_fixture(data, &bytes);
        let loaded = load(data, HASH, 0, 1).unwrap();
        assert!(loaded
            .error
            .as_deref()
            .unwrap()
            .contains("manually preserve"));
        assert!(loaded.token.is_none());
        assert!(save(data, &loaded.ink, 1, None).is_err());
        assert!(recover(data, &loaded.ink, 1, None).is_err());
        assert_eq!(fs::read(path(data)).unwrap(), bytes);
    }
    #[test]
    fn stale_tokens_and_concurrent_creates_never_overwrite() {
        let dir = tempfile::tempdir().unwrap();
        let data = dir.path();
        let barrier = Arc::new(Barrier::new(3));
        let mut threads = Vec::new();
        for _ in 0..2 {
            let root = data.to_owned();
            let barrier = barrier.clone();
            threads.push(std::thread::spawn(move || {
                barrier.wait();
                save(&root, &PageInk::empty(HASH, 0), 1, None)
            }));
        }
        barrier.wait();
        let successes = threads
            .into_iter()
            .map(|t| usize::from(t.join().unwrap().is_ok()))
            .sum::<usize>();
        assert_eq!(successes, 1);
        let before = fs::read(path(data)).unwrap();
        assert!(save(data, &PageInk::empty(HASH, 0), 1, None).is_err());
        assert!(save(data, &PageInk::empty(HASH, 0), 1, Some("stale")).is_err());
        assert_eq!(fs::read(path(data)).unwrap(), before);
    }
    #[test]
    fn stale_lock_and_invalid_payload_leave_original_untouched() {
        let dir = tempfile::tempdir().unwrap();
        let data = dir.path();
        let mut ink = PageInk::empty(HASH, 0);
        let saved = save(data, &ink, 1, None).unwrap();
        let before = fs::read(path(data)).unwrap();
        let lock_path = path(data).with_extension("json.lock");
        fs::write(&lock_path, b"stale generated lock").unwrap();
        assert!(save(data, &ink, 1, Some(&saved.token)).is_err());
        assert!(lock_path.exists());
        fs::remove_file(lock_path).unwrap();
        ink.version = 2;
        assert!(save(data, &ink, 1, Some(&saved.token)).is_err());
        ink.version = 1;
        // Valid geometry whose decimal encoding is larger than the byte ceiling.
        ink.strokes = vec![
            Stroke {
                tool: InkTool::Highlighter,
                color: [255, 255, 255, 255],
                width: 0.012_345_678,
                points: vec![
                    Point {
                        x: 1.234_567_8e-30,
                        y: 1.234_567_8e-30
                    };
                    MAX_POINTS / MAX_STROKES
                ],
            };
            MAX_STROKES
        ];
        assert!(ink.validate(1).is_ok());
        let encoded_len = serde_json::to_vec(&ink).unwrap().len();
        assert!(encoded_len > MAX_BYTES, "fixture bytes: {encoded_len}");
        assert!(save(data, &ink, 1, Some(&saved.token)).is_err());
        assert_eq!(fs::read(path(data)).unwrap(), before);
        assert!(!path(data).with_extension("json.lock").exists());
    }
    #[test]
    fn invalid_paths_and_io_errors_are_not_defaults() {
        let dir = tempfile::tempdir().unwrap();
        assert!(load(dir.path(), "../bad", 0, 1).is_err());
        assert!(load(dir.path(), HASH, 1, 1).is_err());
        fs::create_dir_all(path(dir.path())).unwrap();
        assert!(load(dir.path(), HASH, 0, 1).is_err());
        assert!(save(dir.path(), &PageInk::empty(HASH, 0), 1, None).is_err());
    }
    #[test]
    fn commit_warning_is_not_a_rollback_and_rename_failure_cleans_temp() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("page.json");
        fs::write(&target, b"previous bytes").unwrap();
        let saved = replacement_with_sync(&target, b"committed bytes", |_| {
            bail!("generated directory sync failure")
        })
        .unwrap();
        assert!(saved.durability_warning.unwrap().contains("generated"));
        assert_eq!(
            saved.token,
            format!("{:x}", Sha256::digest(b"committed bytes"))
        );
        assert_eq!(fs::read(&target).unwrap(), b"committed bytes");
        let blocked = dir.path().join("blocked.json");
        fs::create_dir(&blocked).unwrap();
        fs::write(blocked.join("original"), b"untouched").unwrap();
        assert!(replacement(&blocked, b"replacement").is_err());
        assert_eq!(fs::read(blocked.join("original")).unwrap(), b"untouched");
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 2);
    }

    #[test]
    fn cross_process_lock_child() {
        let Some(data) = std::env::var_os("LINUXSHEETS_ANNOTATION_LOCK_FIXTURE") else {
            return;
        };
        let error = save(Path::new(&data), &PageInk::empty(HASH, 0), 1, None).unwrap_err();
        assert!(format!("{error:#}").contains("write lock unavailable"));
    }

    #[test]
    fn cross_process_lock_excludes_other_writers() {
        let dir = tempfile::tempdir().unwrap();
        let target = path(dir.path());
        prepare_directory(target.parent().unwrap()).unwrap();
        let lock = PageLock::acquire(&target).unwrap();
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "annotations::store::tests::cross_process_lock_child",
                "--nocapture",
            ])
            .env("LINUXSHEETS_ANNOTATION_LOCK_FIXTURE", dir.path())
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(!target.exists());
        assert!(target.with_extension("json.lock").exists());
        drop(lock);
        assert!(!target.with_extension("json.lock").exists());
        save(dir.path(), &PageInk::empty(HASH, 0), 1, None).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn private_sidecar_backup_and_directories() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let saved = save(dir.path(), &PageInk::empty(HASH, 0), 1, None).unwrap();
        assert!(saved.durability_warning.is_none());
        let target = path(dir.path());
        for item in [&target, target.parent().unwrap()] {
            assert_eq!(fs::metadata(item).unwrap().permissions().mode() & 0o077, 0);
        }
        fs::write(&target, b"generated invalid JSON").unwrap();
        let loaded = load(dir.path(), HASH, 0, 1).unwrap();
        let recovered = recover(dir.path(), &loaded.ink, 1, loaded.token.as_deref()).unwrap();
        assert_eq!(
            fs::metadata(recovered.backup).unwrap().permissions().mode() & 0o077,
            0
        );
    }
}
