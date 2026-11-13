use crate::pdf;
use anyhow::{bail, Context, Result};
use image::{DynamicImage, ImageFormat, ImageReader};
use std::{
    path::Path,
    sync::atomic::{AtomicBool, Ordering},
};

pub const MAX_IMAGE_PIXELS: u64 = 40_000_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Pdf,
    Png,
    Jpeg,
}

impl Kind {
    pub fn of(path: &Path) -> Option<Self> {
        match path.extension()?.to_str()?.to_ascii_lowercase().as_str() {
            "pdf" => Some(Self::Pdf),
            "png" => Some(Self::Png),
            "jpg" | "jpeg" => Some(Self::Jpeg),
            _ => None,
        }
    }

    pub fn extension(self) -> &'static str {
        match self {
            Self::Pdf => "pdf",
            Self::Png => "png",
            Self::Jpeg => "jpg",
        }
    }
}

pub fn supported(path: &Path) -> bool {
    Kind::of(path).is_some()
}

pub fn read_image(path: &Path) -> Result<DynamicImage> {
    let (width, height) = ImageReader::open(path)?
        .with_guessed_format()?
        .into_dimensions()?;
    if width == 0 || height == 0 || u64::from(width) * u64::from(height) > MAX_IMAGE_PIXELS {
        bail!("Image is empty or exceeds the 40-megapixel safety limit");
    }
    let mut reader = ImageReader::open(path)?.with_guessed_format()?;
    if !matches!(reader.format(), Some(ImageFormat::Png | ImageFormat::Jpeg)) {
        bail!("Only PNG and JPEG images are supported");
    }
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(20_000);
    limits.max_image_height = Some(20_000);
    limits.max_alloc = Some(192 * 1024 * 1024);
    reader.limits(limits);
    reader.decode().context("Image could not be decoded")
}

pub fn info(path: &Path, kind: Kind, cancel: &AtomicBool) -> Result<pdf::PdfInfo> {
    if cancel.load(Ordering::Relaxed) {
        bail!("Operation cancelled");
    }
    if kind == Kind::Pdf {
        return pdf::info(path, cancel);
    }
    let expected = if kind == Kind::Png {
        ImageFormat::Png
    } else {
        ImageFormat::Jpeg
    };
    if ImageReader::open(path)?.with_guessed_format()?.format() != Some(expected) {
        bail!("Image contents do not match the file extension");
    }
    read_image(path)?;
    if cancel.load(Ordering::Relaxed) {
        bail!("Operation cancelled");
    }
    Ok(pdf::PdfInfo {
        pages: 1,
        author: String::new(),
    })
}

pub fn render(
    path: &Path,
    page: u32,
    pixels: u32,
    cancel: &AtomicBool,
) -> Result<image::RgbaImage> {
    let kind = Kind::of(path).context("Unsupported score file extension")?;
    if kind == Kind::Pdf {
        return pdf::render(path, page, pixels, cancel);
    }
    if page != 0 {
        bail!("Standalone image scores have only one page");
    }
    if cancel.load(Ordering::Relaxed) {
        bail!("Operation cancelled");
    }
    let image = read_image(path)?;
    let size = pixels.clamp(256, 4096);
    let image = if image.width() > size || image.height() > size {
        image.thumbnail(size, size)
    } else {
        image
    };
    let mut image = image.into_rgba8();
    for pixel in image.pixels_mut() {
        let alpha = u16::from(pixel[3]);
        for channel in &mut pixel.0[..3] {
            *channel = ((u16::from(*channel) * alpha + 255 * (255 - alpha)) / 255) as u8;
        }
        pixel[3] = 255;
    }
    if cancel.load(Ordering::Relaxed) {
        bail!("Operation cancelled");
    }
    Ok(image)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blank_image_render_has_no_artificial_contrast() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("Blank.png");
        image::RgbImage::from_pixel(100, 140, image::Rgb([255, 255, 255]))
            .save(&path)
            .unwrap();
        let rendered = render(&path, 0, 3000, &AtomicBool::new(false)).unwrap();
        let minimum = rendered.pixels().map(|p| p[0]).min().unwrap();
        let maximum = rendered.pixels().map(|p| p[0]).max().unwrap();
        assert_eq!(minimum, maximum, "Blank image acquired contrast");
    }
}
