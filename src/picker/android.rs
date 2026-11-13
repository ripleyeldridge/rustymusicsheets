use super::{FilePicker, Request};
use anyhow::{bail, Result};
use std::path::PathBuf;

pub(super) struct Backend;

impl FilePicker for Backend {
    fn select(&self, _request: &Request) -> Result<Option<PathBuf>> {
        bail!("The in-app Android document picker is not wired yet. Use your document app's Open with RustyMusicSheets action for a PNG/JPEG. Android document-tree folder access is not implemented.")
    }
}
