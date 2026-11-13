use super::{FilePicker, Request};
use anyhow::{bail, Result};
use std::path::PathBuf;

pub(super) struct Backend;

impl FilePicker for Backend {
    fn select(&self, _request: &Request) -> Result<Option<PathBuf>> {
        bail!("Native file selection is not implemented for {}; paste a path or use drag-and-drop instead.", std::env::consts::OS)
    }
}
