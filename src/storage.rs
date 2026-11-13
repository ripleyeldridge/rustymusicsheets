use crate::instruments::{normalize_manual, Detection, Instrument};
use crate::model::{Entry, Group, Paths, Score};
use anyhow::{bail, Context, Result};
use rusqlite::{params, Connection, OptionalExtension};
use std::{
    os::unix::ffi::{OsStrExt, OsStringExt},
    path::{Path, PathBuf},
    sync::atomic::{AtomicBool, Ordering},
    time::Duration,
};

pub struct Library {
    conn: Connection,
}

impl Library {
    pub fn open(paths: &Paths) -> Result<Self> {
        let conn = Connection::open(paths.data.join("library.sqlite3"))?;
        conn.busy_timeout(Duration::from_millis(300))?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        let version: u32 = conn.pragma_query_value(None, "user_version", |row| row.get(0))?;
        if version > 3 {
            bail!("This library was created by a newer RustyMusicSheets version");
        }
        if version == 0 {
            conn.execute_batch(
                "BEGIN IMMEDIATE;
                 CREATE TABLE scores (
                   id INTEGER PRIMARY KEY, hash TEXT NOT NULL UNIQUE,
                   title TEXT NOT NULL, composer TEXT NOT NULL DEFAULT '',
                   artist TEXT NOT NULL DEFAULT '', genre TEXT NOT NULL DEFAULT '',
                   musical_key TEXT NOT NULL DEFAULT '', tags TEXT NOT NULL DEFAULT '',
                   notes TEXT NOT NULL DEFAULT '', pages INTEGER NOT NULL CHECK(pages>0),
                   last_page INTEGER NOT NULL DEFAULT 0 CHECK(last_page>=0));
                 CREATE TABLE groups (
                   id INTEGER PRIMARY KEY, name TEXT NOT NULL,
                   kind TEXT NOT NULL CHECK(kind IN ('setlist','collection')),
                   UNIQUE(name,kind));
                 CREATE TABLE entries (
                   id INTEGER PRIMARY KEY,
                   group_id INTEGER NOT NULL REFERENCES groups(id) ON DELETE CASCADE,
                   score_id INTEGER NOT NULL REFERENCES scores(id) ON DELETE CASCADE,
                   position INTEGER NOT NULL);
                 CREATE INDEX entries_group ON entries(group_id,position,id);
                 CREATE TABLE bookmarks (
                   score_id INTEGER NOT NULL REFERENCES scores(id) ON DELETE CASCADE,
                   page INTEGER NOT NULL CHECK(page>=0), label TEXT NOT NULL,
                   PRIMARY KEY(score_id,page));
                 PRAGMA user_version=1;
                 COMMIT;",
            )?;
        }
        if version < 2 {
            conn.execute_batch(
                "BEGIN IMMEDIATE;
                 ALTER TABLE scores ADD COLUMN file_path BLOB NOT NULL DEFAULT X'';
                 CREATE TABLE settings (key TEXT PRIMARY KEY, value BLOB NOT NULL);
                 PRAGMA user_version=2;
                 COMMIT;",
            )?;
        }
        if version < 3 {
            if version > 0 {
                let directory = paths.data.join("backups");
                std::fs::create_dir_all(&directory)?;
                let backup = tempfile::Builder::new()
                    .prefix("before-instruments-")
                    .suffix(".sqlite3")
                    .tempfile_in(&directory)?;
                conn.backup(rusqlite::DatabaseName::Main, backup.path(), None)
                    .context("Cannot back up the old library before instrument migration")?;
                backup.as_file().sync_all()?;
                backup.keep()?;
            }
            conn.execute_batch(
                "BEGIN IMMEDIATE;
                 ALTER TABLE scores ADD COLUMN instruments_manual INTEGER NOT NULL DEFAULT 0;
                 CREATE TABLE score_instruments (
                   score_id INTEGER NOT NULL REFERENCES scores(id) ON DELETE CASCADE,
                   name TEXT NOT NULL, variant TEXT NOT NULL DEFAULT '', part TEXT NOT NULL DEFAULT '',
                   PRIMARY KEY(score_id,name,variant,part));
                 CREATE INDEX instruments_name ON score_instruments(name,score_id);
                 CREATE TABLE instrument_detections (
                   score_id INTEGER PRIMARY KEY REFERENCES scores(id) ON DELETE CASCADE,
                   content_hash TEXT NOT NULL, status TEXT NOT NULL, result_json TEXT NOT NULL);
                 PRAGMA user_version=3;
                 COMMIT;",
            )?;
        }
        Ok(Self { conn })
    }

    pub fn scores(&self) -> Result<Vec<Score>> {
        let mut stmt = self.conn.prepare(
            "SELECT id,hash,title,composer,artist,genre,musical_key,tags,notes,pages,last_page,file_path,
             instruments_manual,COALESCE((SELECT status FROM instrument_detections d
             WHERE d.score_id=scores.id AND d.content_hash=scores.hash),'')
             FROM scores ORDER BY title COLLATE NOCASE,id",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok(Score {
                id: r.get(0)?,
                hash: r.get(1)?,
                title: r.get(2)?,
                composer: r.get(3)?,
                artist: r.get(4)?,
                genre: r.get(5)?,
                musical_key: r.get(6)?,
                tags: r.get(7)?,
                notes: r.get(8)?,
                pages: r.get(9)?,
                last_page: r.get(10)?,
                file_path: PathBuf::from(std::ffi::OsString::from_vec(r.get(11)?)),
                instruments: Vec::new(),
                instruments_manual: r.get(12)?,
                detection_status: r.get(13)?,
            })
        })?;
        let mut scores: Vec<Score> = rows.collect::<rusqlite::Result<_>>()?;
        let mut statement = self.conn.prepare(
            "SELECT score_id,name,variant,part FROM score_instruments ORDER BY name,variant,part",
        )?;
        let rows = statement.query_map([], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                Instrument {
                    name: r.get(1)?,
                    variant: r.get(2)?,
                    part: r.get(3)?,
                },
            ))
        })?;
        let mut tags = std::collections::HashMap::<i64, Vec<Instrument>>::new();
        for row in rows {
            let (id, instrument) = row?;
            tags.entry(id).or_default().push(instrument);
        }
        for score in &mut scores {
            score.instruments = tags.remove(&score.id).unwrap_or_default();
        }
        Ok(scores)
    }

    pub fn has_hash(&self, hash: &str) -> Result<bool> {
        Ok(self.conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM scores WHERE hash=?1)",
            [hash],
            |r| r.get(0),
        )?)
    }

    pub fn insert(&self, hash: &str, title: &str, pages: u32, author: &str) -> Result<i64> {
        self.insert_file(hash, title, pages, author, Path::new(""))
    }

    pub fn insert_file(
        &self,
        hash: &str,
        title: &str,
        pages: u32,
        author: &str,
        file: &Path,
    ) -> Result<i64> {
        if !file.as_os_str().is_empty() {
            let existing: Option<i64> = self
                .conn
                .query_row(
                    "SELECT id FROM scores WHERE file_path=?1",
                    [file.as_os_str().as_bytes()],
                    |r| r.get(0),
                )
                .optional()?;
            if let Some(id) = existing {
                let tx = self.conn.unchecked_transaction()?;
                tx.execute(
                    "DELETE FROM score_instruments WHERE score_id=?1 AND
                     EXISTS(SELECT 1 FROM scores WHERE id=?1 AND instruments_manual=0)",
                    [id],
                )?;
                tx.execute(
                    "UPDATE scores SET hash=?1,pages=?2,last_page=MIN(last_page,?2-1) WHERE id=?3",
                    params![hash, pages, id],
                )?;
                tx.commit()?;
                return Ok(id);
            }
        }
        self.conn.execute(
            "INSERT INTO scores(hash,title,pages,composer,file_path) VALUES(?1,?2,?3,?4,?5)",
            params![hash, title, pages, author, file.as_os_str().as_bytes()],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    pub fn save_score(&self, score: &Score) -> Result<()> {
        if score.title.trim().is_empty() {
            bail!("A score needs a title");
        }
        let instruments = normalize_manual(&score.instruments)?;
        let changed_instruments = instruments != self.instruments_for(score.id)?;
        let tx = self.conn.unchecked_transaction()?;
        let changed = tx.execute(
            "UPDATE scores SET title=?1,composer=?2,artist=?3,genre=?4,musical_key=?5,
             tags=?6,notes=?7,instruments_manual=?9 WHERE id=?8",
            params![
                score.title,
                score.composer,
                score.artist,
                score.genre,
                score.musical_key,
                score.tags,
                score.notes,
                score.id,
                score.instruments_manual || changed_instruments
            ],
        )?;
        if changed == 0 {
            bail!("This score no longer exists in the library");
        }
        Self::write_instruments(&tx, score.id, &instruments)?;
        tx.commit()?;
        Ok(())
    }

    pub fn instruments_for(&self, id: i64) -> Result<Vec<Instrument>> {
        let mut statement = self.conn.prepare(
            "SELECT name,variant,part FROM score_instruments WHERE score_id=?1 ORDER BY name,variant,part")?;
        let rows = statement.query_map([id], |r| {
            Ok(Instrument {
                name: r.get(0)?,
                variant: r.get(1)?,
                part: r.get(2)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    fn write_instruments(conn: &Connection, id: i64, instruments: &[Instrument]) -> Result<()> {
        conn.execute("DELETE FROM score_instruments WHERE score_id=?1", [id])?;
        for instrument in instruments {
            conn.execute(
                "INSERT INTO score_instruments(score_id,name,variant,part) VALUES(?1,?2,?3,?4)",
                params![id, instrument.name, instrument.variant, instrument.part],
            )?;
        }
        Ok(())
    }

    pub fn store_detection(
        &self,
        id: i64,
        hash: &str,
        result: &Detection,
        auto_fill: bool,
    ) -> Result<bool> {
        let tx = self.conn.unchecked_transaction()?;
        let current: Option<(String, bool)> = tx
            .query_row(
                "SELECT hash,instruments_manual FROM scores WHERE id=?1",
                [id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        let Some((current, manual)) = current else {
            bail!("Score was removed during detection");
        };
        if current != hash {
            bail!("Score changed during detection; rerun detection");
        }
        let count: i64 = tx.query_row(
            "SELECT COUNT(*) FROM score_instruments WHERE score_id=?1",
            [id],
            |r| r.get(0),
        )?;
        let clear = normalize_manual(
            &result
                .candidates
                .iter()
                .filter(|c| c.clear)
                .map(|c| c.instrument.clone())
                .collect::<Vec<_>>(),
        )?;
        let apply =
            auto_fill && !manual && count == 0 && !clear.is_empty() && result.error.is_none();
        if apply {
            Self::write_instruments(&tx, id, &clear)?;
        }
        let status = if result.error.is_some() {
            "error"
        } else if apply {
            "applied"
        } else if result.candidates.is_empty() {
            "unrecognized"
        } else {
            "review"
        };
        tx.execute("INSERT INTO instrument_detections(score_id,content_hash,status,result_json)
            VALUES(?1,?2,?3,?4) ON CONFLICT(score_id) DO UPDATE SET
            content_hash=excluded.content_hash,status=excluded.status,result_json=excluded.result_json",
            params![id, hash, status, serde_json::to_string(result)?])?;
        tx.commit()?;
        Ok(apply)
    }

    pub fn detection(&self, id: i64, hash: &str) -> Result<Option<Detection>> {
        let json: Option<String> = self.conn.query_row(
            "SELECT result_json FROM instrument_detections WHERE score_id=?1 AND content_hash=?2
             AND EXISTS(SELECT 1 FROM scores WHERE id=?1 AND hash=?2)",
            params![id, hash], |r| r.get(0)).optional()?;
        json.map(|json| Ok(serde_json::from_str(&json)?))
            .transpose()
    }

    pub fn apply_instruments(&self, id: i64, hash: &str, values: &[Instrument]) -> Result<()> {
        let values = normalize_manual(values)?;
        let tx = self.conn.unchecked_transaction()?;
        let changed = tx.execute(
            "UPDATE scores SET instruments_manual=1 WHERE id=?1 AND hash=?2",
            params![id, hash],
        )?;
        if changed == 0 {
            bail!("Score changed or was removed; reopen the detection review");
        }
        Self::write_instruments(&tx, id, &values)?;
        tx.commit()?;
        Ok(())
    }
    pub fn prune_missing(
        &mut self,
        paths: &Paths,
        candidates: &[Score],
        cancel: &AtomicBool,
        validate_root: impl Fn() -> Result<()>,
    ) -> Result<(usize, Option<PathBuf>)> {
        if candidates.is_empty() {
            return Ok((0, None));
        }
        validate_root()?;
        if cancel.load(Ordering::Relaxed) {
            bail!("Rescan cancelled");
        }
        let directory = paths.data.join("backups");
        std::fs::create_dir_all(&directory)
            .context("Cannot create metadata backup directory; no scores were removed")?;
        let backup = tempfile::Builder::new()
            .prefix("before-rescan-")
            .suffix(".sqlite3")
            .tempfile_in(&directory)
            .context("Cannot create metadata backup; no scores were removed")?;
        self.conn
            .backup(rusqlite::DatabaseName::Main, backup.path(), None)
            .context("Cannot back up metadata; no missing scores were removed")?;
        backup.as_file().sync_all()?;
        let (_, backup_path) = backup.keep()?;
        let tx = self.conn.transaction()?;
        let mut removed = 0;
        for score in candidates {
            if cancel.load(Ordering::Relaxed) {
                bail!("Rescan cancelled");
            }
            let path = if score.file_path.as_os_str().is_empty() {
                paths.pdf(&score.hash)
            } else {
                paths.score_dir().join(&score.file_path)
            };
            match std::fs::metadata(&path) {
                Ok(metadata) if metadata.is_file() => continue,
                Ok(_) => {} // A directory is not a backing PDF file.
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => {
                    return Err(error).context("Cannot verify missing PDF; cleanup cancelled")
                }
            }
            removed += tx.execute(
                "DELETE FROM scores WHERE id=?1 AND hash=?2 AND file_path=?3",
                params![score.id, score.hash, score.file_path.as_os_str().as_bytes()],
            )?;
        }
        validate_root()?;
        if cancel.load(Ordering::Relaxed) {
            bail!("Rescan cancelled");
        }
        tx.commit()?;
        Ok((removed, Some(backup_path)))
    }

    pub fn pdf_path(&self, paths: &Paths, hash: &str) -> Result<PathBuf> {
        let raw: Vec<u8> =
            self.conn
                .query_row("SELECT file_path FROM scores WHERE hash=?1", [hash], |r| {
                    r.get(0)
                })?;
        if raw.is_empty() {
            return Ok(paths.pdf(hash));
        }
        let relative = PathBuf::from(std::ffi::OsString::from_vec(raw));
        if relative
            .components()
            .any(|c| !matches!(c, std::path::Component::Normal(_)))
        {
            bail!("Invalid library-relative file path");
        }
        Ok(paths.score_dir().join(relative))
    }

    pub fn music_folder(&self) -> Result<Option<PathBuf>> {
        let raw: Option<Vec<u8>> = self
            .conn
            .query_row(
                "SELECT value FROM settings WHERE key='music_folder'",
                [],
                |r| r.get(0),
            )
            .optional()?;
        Ok(raw.map(|bytes| PathBuf::from(std::ffi::OsString::from_vec(bytes))))
    }

    pub fn set_music_folder(&mut self, folder: &Path, files: &[(i64, PathBuf)]) -> Result<()> {
        let tx = self.conn.transaction()?;
        for (id, relative) in files {
            tx.execute(
                "UPDATE scores SET file_path=?1 WHERE id=?2",
                params![relative.as_os_str().as_bytes(), id],
            )?;
        }
        tx.execute(
            "INSERT INTO settings(key,value) VALUES('music_folder',?1)
                    ON CONFLICT(key) DO UPDATE SET value=excluded.value",
            [folder.as_os_str().as_bytes()],
        )?;
        tx.commit()?;
        Ok(())
    }

    pub fn relocate(&self, hash: &str, relative: &Path) -> Result<()> {
        self.conn.execute(
            "UPDATE scores SET file_path=?1 WHERE hash=?2",
            params![relative.as_os_str().as_bytes(), hash],
        )?;
        Ok(())
    }

    pub fn remember_page(&self, score: i64, page: u32) -> Result<()> {
        let pages: u32 =
            self.conn
                .query_row("SELECT pages FROM scores WHERE id=?1", [score], |r| {
                    r.get(0)
                })?;
        if page >= pages {
            bail!("Page is outside this score");
        }
        self.conn.execute(
            "UPDATE scores SET last_page=?1 WHERE id=?2",
            params![page, score],
        )?;
        Ok(())
    }

    pub fn groups(&self) -> Result<Vec<Group>> {
        let mut stmt = self
            .conn
            .prepare("SELECT id,name,kind FROM groups ORDER BY kind,name COLLATE NOCASE")?;
        let rows = stmt.query_map([], |r| {
            Ok(Group {
                id: r.get(0)?,
                name: r.get(1)?,
                kind: r.get(2)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    pub fn create_group(&self, name: &str, kind: &str) -> Result<i64> {
        if name.trim().is_empty() {
            bail!("Enter a group name");
        }
        self.conn.execute(
            "INSERT INTO groups(name,kind) VALUES(?1,?2)",
            params![name.trim(), kind],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    pub fn rename_group(&self, group: i64, name: &str) -> Result<()> {
        let name = name.trim();
        if name.is_empty() {
            bail!("Enter a group name");
        }
        let changed = self
            .conn
            .execute(
                "UPDATE groups SET name=?1 WHERE id=?2",
                params![name, group],
            )
            .context("Cannot rename group; that name may already be in use")?;
        if changed == 0 {
            bail!("The selected group no longer exists");
        }
        Ok(())
    }

    pub fn duplicate_group(&mut self, group: i64, name: &str) -> Result<i64> {
        let name = name.trim();
        if name.is_empty() {
            bail!("Enter a name for the copy");
        }
        let tx = self.conn.transaction()?;
        let kind: String = tx
            .query_row("SELECT kind FROM groups WHERE id=?1", [group], |r| r.get(0))
            .context("The selected group no longer exists")?;
        tx.execute(
            "INSERT INTO groups(name,kind) VALUES(?1,?2)",
            params![name, kind],
        )
        .context("Cannot duplicate group; that name may already be in use")?;
        let copy = tx.last_insert_rowid();
        tx.execute(
            "INSERT INTO entries(group_id,score_id,position)
             SELECT ?1,score_id,position FROM entries WHERE group_id=?2 ORDER BY position,id",
            params![copy, group],
        )?;
        tx.commit()?;
        Ok(copy)
    }

    pub fn delete_group(&self, group: i64) -> Result<()> {
        self.conn
            .execute("DELETE FROM groups WHERE id=?1", [group])?;
        Ok(())
    }

    pub fn entries(&self, group: i64) -> Result<Vec<Entry>> {
        let mut stmt = self
            .conn
            .prepare("SELECT id,score_id FROM entries WHERE group_id=?1 ORDER BY position,id")?;
        let rows = stmt.query_map([group], |r| {
            Ok(Entry {
                id: r.get(0)?,
                score_id: r.get(1)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    pub fn add_entry(&self, group: i64, score: i64) -> Result<()> {
        let kind: String =
            self.conn
                .query_row("SELECT kind FROM groups WHERE id=?1", [group], |r| r.get(0))?;
        if kind == "collection" {
            let exists: bool = self.conn.query_row(
                "SELECT EXISTS(SELECT 1 FROM entries WHERE group_id=?1 AND score_id=?2)",
                params![group, score],
                |r| r.get(0),
            )?;
            if exists {
                return Ok(());
            }
        }
        self.conn.execute(
            "INSERT INTO entries(group_id,score_id,position)
             SELECT ?1,?2,COALESCE(MAX(position),-1)+1 FROM entries WHERE group_id=?1",
            params![group, score],
        )?;
        Ok(())
    }

    pub fn remove_entry(&self, entry: i64) -> Result<()> {
        self.conn
            .execute("DELETE FROM entries WHERE id=?1", [entry])?;
        Ok(())
    }

    pub fn move_entry(&mut self, group: i64, index: usize, offset: isize) -> Result<()> {
        let mut entries = self.entries(group)?;
        let target = index.checked_add_signed(offset);
        let Some(target) = target.filter(|&target| target < entries.len()) else {
            return Ok(());
        };
        if index >= entries.len() {
            bail!("Entry no longer exists");
        }
        entries.swap(index, target);
        let tx = self.conn.transaction()?;
        for (position, entry) in entries.iter().enumerate() {
            tx.execute(
                "UPDATE entries SET position=?1 WHERE id=?2",
                params![position as i64, entry.id],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    pub fn bookmarks(&self, score: i64) -> Result<Vec<(u32, String)>> {
        let mut stmt = self
            .conn
            .prepare("SELECT page,label FROM bookmarks WHERE score_id=?1 ORDER BY page")?;
        let rows = stmt.query_map([score], |r| Ok((r.get(0)?, r.get(1)?)))?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    pub fn toggle_bookmark(&self, score: i64, page: u32) -> Result<()> {
        let pages: u32 = self
            .conn
            .query_row("SELECT pages FROM scores WHERE id=?1", [score], |r| {
                r.get(0)
            })
            .context("Score no longer exists")?;
        if page >= pages {
            bail!("Bookmark is outside this score");
        }
        let exists: Option<String> = self
            .conn
            .query_row(
                "SELECT label FROM bookmarks WHERE score_id=?1 AND page=?2",
                params![score, page],
                |r| r.get(0),
            )
            .optional()?;
        if exists.is_some() {
            self.conn.execute(
                "DELETE FROM bookmarks WHERE score_id=?1 AND page=?2",
                params![score, page],
            )?;
        } else {
            self.conn.execute(
                "INSERT INTO bookmarks VALUES(?1,?2,?3)",
                params![score, page, format!("Page {}", page + 1)],
            )?;
        }
        Ok(())
    }
}
