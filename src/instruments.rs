use serde::{Deserialize, Serialize};
pub const DETECTION_TIME_WARNING: &str = "Instrument detection may take a while for large libraries, especially scanned PDFs and images. You can keep reading or cancel detection from the status bar.";

#[derive(Clone, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct Instrument {
    pub name: String,
    #[serde(default)]
    pub variant: String,
    #[serde(default)]
    pub part: String,
}

impl Instrument {
    pub fn label(&self) -> String {
        let mut label = self.name.clone();
        if !self.variant.is_empty() {
            label.push_str(&format!(" ({})", self.variant));
        }
        if !self.part.is_empty() {
            label.push_str(&format!(" · part {}", self.part));
        }
        label
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct DetectionOptions {
    pub pages: u32,
    pub language: String,
}

impl Default for DetectionOptions {
    fn default() -> Self {
        Self {
            pages: 2,
            language: "eng".into(),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Candidate {
    pub instrument: Instrument,
    pub evidence: String,
    pub page: u32,
    pub method: String,
    pub clear: bool,
    pub ocr_confidence: Option<f32>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Detection {
    #[serde(default)]
    pub file_size: u64,
    #[serde(default)]
    pub modified_ns: Option<u64>,
    pub candidates: Vec<Candidate>,
    pub warnings: Vec<String>,
    pub pages: u32,
    pub error: Option<String>,
}

pub mod classify;
pub mod extract;
pub mod job;
pub mod review;
pub use classify::{catalogue, normalize_manual};
