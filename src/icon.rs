use anyhow::Result;

pub const APP_ID: &str = "rustymusicsheets";

/// Embedded so window/task-switcher icons also work when run outside a package.
pub fn application_icon() -> Result<eframe::egui::IconData> {
    let image = image::load_from_memory(include_bytes!(
        "../assets/icons/256x256/rustymusicsheets.png"
    ))?
    .into_rgba8();
    let (width, height) = image.dimensions();
    Ok(eframe::egui::IconData {
        rgba: image.into_raw(),
        width,
        height,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embedded_window_icon_is_valid_and_matches_desktop_identity() {
        let icon = application_icon().unwrap();
        assert_eq!((icon.width, icon.height), (256, 256));
        assert_eq!(icon.rgba.len(), 256 * 256 * 4);
        assert!(icon.rgba.chunks_exact(4).any(|pixel| pixel[3] == 0));
        assert!(icon.rgba.chunks_exact(4).any(|pixel| pixel[3] == 255));
        let desktop = include_str!("../assets/rustymusicsheets.desktop");
        assert!(desktop.lines().any(|line| line == format!("Icon={APP_ID}")));
        assert!(desktop
            .lines()
            .any(|line| line == format!("StartupWMClass={APP_ID}")));
    }
}
