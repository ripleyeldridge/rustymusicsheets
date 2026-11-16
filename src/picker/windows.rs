use super::{FilePicker, Request, SelectionKind};
use anyhow::Result;
use std::path::PathBuf;

pub(super) struct Backend;

impl FilePicker for Backend {
    fn select(&self, request: &Request) -> Result<Option<PathBuf>> {
        let mut dialog = rfd::FileDialog::new()
            .set_title(format!("RustyMusicSheets — {}", request.kind.title()))
            .set_directory(&request.directory);
        if let Some(name) = request.filename.as_ref().and_then(|name| name.to_str()) {
            dialog = dialog.set_file_name(name);
        }
        if !request.kind.extensions().is_empty() {
            dialog = dialog.add_filter(request.kind.title(), request.kind.extensions());
        }
        // Native dialogs return paths directly; no shell output or UTF-8 conversion.
        Ok(if request.kind == SelectionKind::Folder {
            dialog.pick_folder()
        } else if request.kind.is_save() {
            dialog.save_file()
        } else {
            dialog.pick_file()
        })
    }
}
