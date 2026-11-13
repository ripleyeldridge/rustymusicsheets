//! External references are not managed-library paths. Android URIs must be resolved.
use crate::document::Kind;
use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::{
    fs::File,
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::atomic::{AtomicBool, Ordering},
};

pub const MAX_EXTERNAL_DOCUMENT_BYTES: u64 = 512 * 1024 * 1024;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct ContentUri(String);

impl ContentUri {
    pub fn parse(value: &str) -> Result<Self> {
        if !value.starts_with("content://")
            || value.chars().any(|c| c.is_control() || c.is_whitespace())
        {
            bail!("Expected an Android content:// document URI");
        }
        let parsed = url::Url::parse(value).context("Invalid document URI")?;
        if parsed.host_str().is_none_or(str::is_empty)
            || parsed.fragment().is_some()
            || parsed.port().is_some()
        {
            bail!("Invalid Android document authority or fragment");
        }
        // Keep the provider's original encoding; do not normalize/decode into a path.
        Ok(Self(value.to_owned()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for ContentUri {
    type Error = anyhow::Error;
    fn try_from(value: String) -> Result<Self> {
        Self::parse(&value)
    }
}

impl From<ContentUri> for String {
    fn from(value: ContentUri) -> Self {
        value.0
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "value")]
pub enum DocumentRef {
    Path(PathBuf),
    Content(ContentUri),
}

impl DocumentRef {
    pub fn as_path(&self) -> Option<&Path> {
        match self {
            Self::Path(path) => Some(path),
            Self::Content(_) => None,
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct Metadata {
    pub display_name: Option<String>,
    pub mime_type: Option<String>,
    pub size: Option<u64>,
}

impl Metadata {
    fn import_name(&self) -> Result<String> {
        let name = self
            .display_name
            .as_deref()
            .unwrap_or("Imported score")
            .trim();
        if name.is_empty()
            || name == "."
            || name == ".."
            || name
                .chars()
                .any(|c| c.is_control() || matches!(c, '/' | '\\' | ':'))
            || name.len() > 240
        {
            bail!("Document provider returned an unsafe display name");
        }
        let extension_kind = Kind::of(Path::new(name));
        let mime_kind = match self
            .mime_type
            .as_deref()
            .map(|m| {
                m.split(';')
                    .next()
                    .unwrap_or_default()
                    .trim()
                    .to_ascii_lowercase()
            })
            .as_deref()
        {
            None | Some("" | "application/octet-stream") => None,
            Some("application/pdf") => Some(Kind::Pdf),
            Some("image/png") => Some(Kind::Png),
            Some("image/jpeg") => Some(Kind::Jpeg),
            Some(_) => bail!("Unsupported document MIME type; expected PDF, PNG, or JPEG"),
        };
        if extension_kind.zip(mime_kind).is_some_and(|(a, b)| a != b) {
            bail!("Document name and MIME type disagree");
        }
        let kind = mime_kind
            .or(extension_kind)
            .context("Document format is unknown")?;
        let mut path = PathBuf::from(name);
        if extension_kind.is_none() {
            path.set_extension(kind.extension());
        }
        Ok(path.to_string_lossy().into_owned())
    }
}

pub trait DocumentAccess: Send + Sync {
    fn metadata(&self, source: &DocumentRef) -> Result<Metadata>;
    fn open(&self, source: &DocumentRef) -> Result<Box<dyn Read + Send>>;
}

pub struct LocalDocuments;

impl DocumentAccess for LocalDocuments {
    fn metadata(&self, source: &DocumentRef) -> Result<Metadata> {
        let path = source
            .as_path()
            .context("A content URI needs an Android document resolver")?;
        let metadata = std::fs::symlink_metadata(path)?;
        if !metadata.is_file() {
            bail!("Document is not a regular file");
        }
        Ok(Metadata {
            display_name: path.file_name().map(|n| n.to_string_lossy().into_owned()),
            size: Some(metadata.len()),
            ..Default::default()
        })
    }

    fn open(&self, source: &DocumentRef) -> Result<Box<dyn Read + Send>> {
        let path = source
            .as_path()
            .context("A content URI needs an Android document resolver")?;
        Ok(Box::new(File::open(path)?))
    }
}

pub struct StagedDocument {
    pub path: PathBuf,
    _temporary: tempfile::TempDir,
}

/// Temporary private copy; disposal removes it even when validation/import fails.
pub fn stage(
    source: &DocumentRef,
    access: &dyn DocumentAccess,
    cache: &Path,
    cancel: &AtomicBool,
) -> Result<StagedDocument> {
    stage_with_limit(source, access, cache, cancel, MAX_EXTERNAL_DOCUMENT_BYTES)
}

fn stage_with_limit(
    source: &DocumentRef,
    access: &dyn DocumentAccess,
    cache: &Path,
    cancel: &AtomicBool,
    limit: u64,
) -> Result<StagedDocument> {
    let check_cancel = || -> Result<()> {
        if cancel.load(Ordering::Relaxed) {
            bail!("Document import cancelled");
        }
        Ok(())
    };
    check_cancel()?;
    let metadata = access.metadata(source)?;
    let name = metadata.import_name()?;
    if metadata.size.is_some_and(|size| size > limit) {
        bail!("External document exceeds the import size limit");
    }
    check_cancel()?;
    let mut input = access.open(source)?;
    let temporary = tempfile::tempdir_in(cache)?;
    let path = temporary.path().join(name);
    let mut output = File::create(&path)?;
    let mut copied = 0u64;
    let mut buffer = [0; 64 * 1024];
    loop {
        check_cancel()?;
        let count = match input.read(&mut buffer) {
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            result => result?,
        };
        if count == 0 {
            break;
        }
        copied += count as u64;
        if copied > limit {
            bail!("External document exceeds the import size limit");
        }
        output.write_all(&buffer[..count])?;
    }
    check_cancel()?;
    if metadata.size.is_some_and(|size| size != copied) {
        bail!("Document size changed during import");
    }
    output.sync_all()?;
    drop(output);
    Ok(StagedDocument {
        path,
        _temporary: temporary,
    })
}

#[cfg(test)]
mod tests;
