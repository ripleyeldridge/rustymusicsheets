use super::{FilePicker, Request, SelectionKind, SCORE_EXTENSIONS};
use anyhow::Result;
use std::path::PathBuf;

pub(super) struct Backend;

impl FilePicker for Backend {
    fn select(&self, request: &Request) -> Result<Option<PathBuf>> {
        let dialog = rfd::FileDialog::new()
            .set_title("RustyMusicSheets — Select music")
            .set_directory(&request.directory);
        // Native dialogs return paths directly; no shell output or UTF-8 conversion.
        Ok(match request.kind {
            SelectionKind::Folder => dialog.pick_folder(),
            SelectionKind::Score => dialog.add_filter("Scores", SCORE_EXTENSIONS).pick_file(),
        })
    }
}
