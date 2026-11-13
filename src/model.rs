use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Clone, Debug, Default)]
pub struct Score {
    pub id: i64,
    pub hash: String,
    pub title: String,
    pub composer: String,
    pub artist: String,
    pub genre: String,
    pub musical_key: String,
    pub tags: String,
    pub notes: String,
    pub pages: u32,
    pub last_page: u32,
    pub file_path: PathBuf,
    pub instruments: Vec<crate::instruments::Instrument>,
    pub instruments_manual: bool,
    pub detection_status: String,
}

impl Score {
    pub fn metadata_matches(&self, other: &Self) -> bool {
        self.title == other.title
            && self.composer == other.composer
            && self.artist == other.artist
            && self.genre == other.genre
            && self.musical_key == other.musical_key
            && self.tags == other.tags
            && self.notes == other.notes
            && self.instruments == other.instruments
            && self.instruments_manual == other.instruments_manual
    }
    pub fn matches(&self, query: &str) -> bool {
        self.matches_display(query, &self.title)
    }

    pub fn matches_display(&self, query: &str, display_title: &str) -> bool {
        let fields: [&str; 8] = [
            &self.title,
            &self.composer,
            &self.artist,
            &self.genre,
            &self.musical_key,
            &self.tags,
            &self.notes,
            display_title,
        ];
        query.split_whitespace().all(|word| {
            let word = word.to_lowercase();
            fields
                .iter()
                .any(|field| field.to_lowercase().contains(&word))
                || self
                    .instruments
                    .iter()
                    .any(|instrument| instrument.label().to_lowercase().contains(&word))
        })
    }
}

#[derive(Clone, Debug)]
pub struct Group {
    pub id: i64,
    pub name: String,
    pub kind: String,
}

#[derive(Clone, Debug)]
pub struct Entry {
    pub id: i64,
    pub score_id: i64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Preferences {
    pub next_key: String,
    pub previous_key: String,
    pub recursive: bool,
    #[serde(default)]
    pub display_names: crate::display_names::DisplayNames,
    #[serde(default)]
    pub keep_awake: bool,
    #[serde(default)]
    pub detect_on_import: bool,
    #[serde(default)]
    pub detection_options: crate::instruments::DetectionOptions,
}

impl Default for Preferences {
    fn default() -> Self {
        Self {
            next_key: "PageDown".into(),
            previous_key: "PageUp".into(),
            recursive: true,
            display_names: crate::display_names::DisplayNames::default(),
            keep_awake: false,
            detect_on_import: false,
            detection_options: crate::instruments::DetectionOptions::default(),
        }
    }
}

#[derive(Clone, Debug)]
pub struct Paths {
    pub data: PathBuf,
    pub cache: PathBuf,
    pub config: PathBuf,
    pub music: Option<PathBuf>,
}

impl Paths {
    pub fn discover() -> anyhow::Result<Self> {
        let dirs = directories::ProjectDirs::from("org", "RustyMusicSheets", "rustymusicsheets")
            .ok_or_else(|| anyhow::anyhow!("Cannot find your home/data directories"))?;
        let mut paths = Self {
            data: dirs.data_dir().to_owned(),
            cache: dirs.cache_dir().to_owned(),
            config: dirs.config_dir().to_owned(),
            music: None,
        };
        paths.create()?;
        paths.music = crate::storage::Library::open(&paths)?.music_folder()?;
        Ok(paths)
    }

    pub fn create(&self) -> anyhow::Result<()> {
        std::fs::create_dir_all(self.data.join("scores"))?;
        std::fs::create_dir_all(&self.cache)?;
        std::fs::create_dir_all(&self.config)?;
        Ok(())
    }

    pub fn pdf(&self, hash: &str) -> PathBuf {
        self.score_dir().join(format!("{hash}.pdf"))
    }

    pub fn score_dir(&self) -> PathBuf {
        self.music
            .clone()
            .unwrap_or_else(|| self.data.join("scores"))
    }

    pub fn preferences(&self) -> anyhow::Result<Preferences> {
        let path = self.config.join("preferences.json");
        match std::fs::read(path) {
            Ok(data) => Ok(serde_json::from_slice(&data)?),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Preferences::default()),
            Err(e) => Err(e.into()),
        }
    }

    pub fn save_preferences(&self, prefs: &Preferences) -> anyhow::Result<()> {
        use std::io::Write;
        let mut file = tempfile::NamedTempFile::new_in(&self.config)?;
        file.write_all(&serde_json::to_vec_pretty(prefs)?)?;
        file.as_file().sync_all()?;
        file.persist(self.config.join("preferences.json"))?;
        Ok(())
    }
}
