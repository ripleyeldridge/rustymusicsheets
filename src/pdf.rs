use anyhow::{bail, Context, Result};
use std::{
    fs::File,
    io::Read,
    path::Path,
    process::{Command, Stdio},
    sync::atomic::{AtomicBool, Ordering},
    time::{Duration, Instant},
};
use wait_timeout::ChildExt;

#[derive(Debug)]
pub struct PdfInfo {
    pub pages: u32,
    pub author: String,
}

pub fn run(command: &mut Command, cancel: &AtomicBool, timeout: Duration) -> Result<Vec<u8>> {
    let mut stdout = tempfile::tempfile()?;
    let mut stderr = tempfile::tempfile()?;
    let mut child = command
        .stdin(Stdio::null())
        .stdout(stdout.try_clone()?)
        .stderr(stderr.try_clone()?)
        .spawn()
        .context("Could not start helper program (install poppler-utils for PDF support)")?;
    let start = Instant::now();
    let status = loop {
        if cancel.load(Ordering::Relaxed) || start.elapsed() > timeout {
            let _ = child.kill();
            let _ = child.wait();
            bail!("Operation cancelled or timed out");
        }
        match child.wait_timeout(Duration::from_millis(50)) {
            Ok(Some(status)) => break status,
            Ok(None) => {}
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(error.into());
            }
        }
    };
    if !status.success() {
        let error = read_limited(&mut stderr)?;
        bail!("Helper failed: {}", String::from_utf8_lossy(&error).trim());
    }
    read_limited(&mut stdout)
}

fn read_limited(file: &mut File) -> Result<Vec<u8>> {
    use std::io::{Seek, SeekFrom};
    file.seek(SeekFrom::Start(0))?;
    let mut data = Vec::new();
    file.take(2 * 1024 * 1024).read_to_end(&mut data)?;
    Ok(data)
}

pub fn info(path: &Path, cancel: &AtomicBool) -> Result<PdfInfo> {
    let output = run(
        Command::new("pdfinfo").env("LC_ALL", "C").arg(path),
        cancel,
        Duration::from_secs(30),
    )?;
    parse_info(&String::from_utf8_lossy(&output))
}

pub fn parse_info(output: &str) -> Result<PdfInfo> {
    let mut pages = None;
    let mut author = String::new();
    for line in output.lines() {
        if let Some(value) = line.strip_prefix("Pages:") {
            pages = value.trim().parse::<u32>().ok();
        }
        if let Some(value) = line.strip_prefix("Author:") {
            author = value.trim().to_owned();
        }
        if let Some(value) = line.strip_prefix("Encrypted:") {
            if value.trim().starts_with("yes") {
                bail!("Encrypted PDFs are not supported; import an unencrypted copy");
            }
        }
    }
    let pages = pages
        .filter(|&p| p > 0)
        .context("PDF has no readable pages")?;
    Ok(PdfInfo { pages, author })
}

pub fn render(
    path: &Path,
    page: u32,
    pixels: u32,
    cancel: &AtomicBool,
) -> Result<image::RgbaImage> {
    let temp = tempfile::tempdir()?;
    let prefix = temp.path().join("page");
    run(
        Command::new("pdftoppm")
            .arg("-f")
            .arg((page + 1).to_string())
            .arg("-l")
            .arg((page + 1).to_string())
            .args(["-singlefile", "-scale-to"])
            .arg(pixels.clamp(256, 4096).to_string())
            .arg("-png")
            .arg(path)
            .arg(&prefix),
        cancel,
        Duration::from_secs(30),
    )?;
    let file = prefix.with_extension("png");
    if std::fs::metadata(&file)?.len() > 80 * 1024 * 1024 {
        bail!("Rendered page exceeds the image size limit");
    }
    Ok(image::open(file)?.into_rgba8())
}
