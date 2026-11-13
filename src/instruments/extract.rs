use super::{classify, Detection, DetectionOptions};
use crate::{document, pdf};
use anyhow::{bail, Context, Result};
use std::{
    collections::BTreeMap,
    path::Path,
    process::Command,
    sync::atomic::{AtomicBool, Ordering},
    time::Duration,
};

pub fn parse_tsv(tsv: &str, page: u32, method: &str, result: &mut Detection) -> Result<()> {
    if !tsv
        .lines()
        .next()
        .is_some_and(|line| line.starts_with("level\t"))
    {
        bail!("Tesseract returned invalid TSV output");
    }
    type Line = (Vec<String>, f32, usize);
    let mut lines = BTreeMap::<(u32, u32, u32, u32), Line>::new();
    for row in tsv.lines().skip(1).take(20_000) {
        let fields: Vec<_> = row.splitn(12, '\t').collect();
        if fields.len() < 12 || fields[0] != "5" || fields[11].trim().is_empty() {
            continue;
        }
        let key = (
            fields[1].parse()?,
            fields[2].parse()?,
            fields[3].parse()?,
            fields[4].parse()?,
        );
        let confidence: f32 = fields[10].parse()?;
        if !confidence.is_finite() || confidence < 0.0 {
            continue;
        }
        let line = lines.entry(key).or_default();
        line.0.push(fields[11].trim().to_owned());
        line.1 += confidence.clamp(0.0, 100.0);
        line.2 += 1;
    }
    for (_, (words, confidence, count)) in lines.into_iter().take(80) {
        classify::line(
            &words.join(" "),
            page,
            method,
            Some(confidence / count as f32),
            result,
        );
    }
    Ok(())
}

pub fn detect(
    path: &Path,
    pages: u32,
    options: &DetectionOptions,
    cancel: &AtomicBool,
    mut progress: impl FnMut(String),
) -> Result<Detection> {
    if options.language.is_empty()
        || options.language.len() > 80
        || !options
            .language
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '+')
    {
        bail!("OCR language must be a Tesseract language name such as eng or eng+deu");
    }
    let kind = document::Kind::of(path).context("Unsupported score format")?;
    let limit = pages.min(options.pages.clamp(1, 6));
    let mut result = Detection::default();
    let metadata = std::fs::metadata(path)?;
    result.file_size = metadata.len();
    result.modified_ns = metadata
        .modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .and_then(|d| u64::try_from(d.as_nanos()).ok());
    for page in 0..limit {
        if cancel.load(Ordering::Relaxed) {
            bail!("Detection cancelled");
        }
        let mut extracted = Detection::default();
        if kind == document::Kind::Pdf {
            progress(format!("Page {}/{}: extracting PDF text", page + 1, limit));
            match pdf::run(
                Command::new("pdftotext")
                    .args(["-layout", "-f"])
                    .arg((page + 1).to_string())
                    .arg("-l")
                    .arg((page + 1).to_string())
                    .arg(path)
                    .arg("-"),
                cancel,
                Duration::from_secs(20),
            ) {
                Ok(text) => classify::text(
                    &String::from_utf8_lossy(&text),
                    page + 1,
                    "PDF text",
                    &mut extracted,
                ),
                Err(error) => extracted
                    .warnings
                    .push(format!("Page {} PDF text: {error:#}", page + 1)),
            }
        }
        let needs_ocr = !extracted.candidates.iter().any(|candidate| candidate.clear);
        merge(&mut result, extracted);
        if needs_ocr {
            progress(format!(
                "Page {}/{}: OCR of headings and staff labels",
                page + 1,
                limit
            ));
            let image = document::render(path, page, 3000, cancel)?;
            let (width, height) = image.dimensions();
            let top = (height * 35 / 100).max(1);
            let left = (width * 30 / 100).max(1);
            let regions = [
                (
                    "Tesseract header",
                    image::imageops::crop_imm(&image, 0, 0, width, top).to_image(),
                ),
                (
                    "Tesseract staff labels",
                    image::imageops::crop_imm(&image, 0, height / 4, left, height - height / 4)
                        .to_image(),
                ),
            ];
            let temp = tempfile::tempdir()?;
            for (index, (method, image)) in regions.into_iter().enumerate() {
                if cancel.load(Ordering::Relaxed) {
                    bail!("Detection cancelled");
                }
                let image = image::imageops::grayscale(&image);
                // Solid regions contain no text; Tesseract can be very slow on them.
                if image.pixels().all(|pixel| pixel == image.get_pixel(0, 0)) {
                    continue;
                }
                let region = temp.path().join(format!("region-{index}.png"));
                image.save(&region)?;
                let output = pdf::run(
                    Command::new("tesseract")
                        .env("OMP_THREAD_LIMIT", "1")
                        .arg(&region)
                        .args(["stdout", "-l"])
                        .arg(&options.language)
                        .args(["--psm", "11", "tsv"]),
                    cancel,
                    Duration::from_secs(45),
                );
                match output.and_then(|text| {
                    parse_tsv(
                        &String::from_utf8_lossy(&text),
                        page + 1,
                        method,
                        &mut result,
                    )
                }) {
                    Ok(()) => {}
                    Err(_) if cancel.load(Ordering::Relaxed) => bail!("Detection cancelled"),
                    Err(error) => {
                        let message = format!(
                            "Page {} OCR: {error:#}. Check tesseract-ocr and language data.",
                            page + 1
                        );
                        if result.warnings.len() < 40 {
                            result.warnings.push(message.clone());
                        }
                        result.error = Some(message);
                    }
                }
            }
        }
        result.pages += 1;
    }
    Ok(result)
}

fn merge(result: &mut Detection, other: Detection) {
    for candidate in other.candidates {
        if let Some(existing) = result
            .candidates
            .iter_mut()
            .find(|old| old.instrument == candidate.instrument)
        {
            if candidate.clear && !existing.clear {
                *existing = candidate;
            }
        } else if result.candidates.len() < 32 {
            result.candidates.push(candidate);
        }
    }
    for warning in other.warnings {
        if result.warnings.len() < 40 && !result.warnings.contains(&warning) {
            result.warnings.push(warning);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn tsv_confidence_and_lines_are_preserved() {
        let tsv = "level\tpage_num\tblock_num\tpar_num\tline_num\tword_num\tleft\ttop\twidth\theight\tconf\ttext\n\
                   5\t1\t1\t1\t1\t1\t1\t1\t20\t20\t95\tVioloncello\n\
                   5\t1\t1\t1\t1\t2\t30\t1\t10\t20\t85\tII\n";
        let mut result = Detection::default();
        parse_tsv(tsv, 2, "OCR", &mut result).unwrap();
        assert_eq!(result.candidates[0].instrument.name, "Cello");
        assert_eq!(result.candidates[0].instrument.part, "2");
        assert_eq!(result.candidates[0].ocr_confidence, Some(90.0));
        assert!(parse_tsv("not TSV", 1, "OCR", &mut result).is_err());
    }
}
