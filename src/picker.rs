use std::{
    path::{Path, PathBuf},
    process::Command,
};

pub fn home_directory() -> PathBuf {
    directories::BaseDirs::new()
        .map(|dirs| dirs.home_dir().to_owned())
        .filter(|home| home.is_dir())
        .unwrap_or_else(|| PathBuf::from("/"))
}

pub fn initial_directory(preferred: &Path, home: &Path) -> PathBuf {
    if preferred.is_dir() {
        preferred.canonicalize().unwrap_or_else(|_| home.to_owned())
    } else if preferred.is_file() {
        preferred
            .canonicalize()
            .ok()
            .as_deref()
            .and_then(Path::parent)
            .filter(|parent| parent.is_dir())
            .unwrap_or(home)
            .to_owned()
    } else if home.is_dir() {
        home.canonicalize().unwrap_or_else(|_| PathBuf::from("/"))
    } else {
        PathBuf::from("/")
    }
}

pub fn command(folder: bool, preferred: &Path, home: &Path) -> Command {
    let directory = initial_directory(preferred, home);
    let mut filename = directory.as_os_str().to_owned();
    filename.push(std::path::MAIN_SEPARATOR_STR);
    let mut command = Command::new("zenity");
    command
        .current_dir(&directory)
        .args([
            "--file-selection",
            "--title=RustyMusicSheets — Select music",
        ])
        .arg("--filename")
        .arg(filename);
    if folder {
        command.arg("--directory");
    } else {
        command.arg("--file-filter=Scores | *.pdf *.PDF *.png *.PNG *.jpg *.JPG *.jpeg *.JPEG");
    }
    command
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_directory_falls_back_to_home_without_creating_it() {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path().join("Home");
        std::fs::create_dir(&home).unwrap();
        let missing = temp.path().join("Missing library");
        assert_eq!(initial_directory(&missing, &home), home);
        assert!(!missing.exists());
        let command = command(true, &missing, &home);
        assert_eq!(command.get_current_dir(), Some(home.as_path()));
        let args: Vec<_> = command
            .get_args()
            .map(|s| s.to_string_lossy().into_owned())
            .collect();
        assert!(args.contains(&format!("{}/", home.display())));
        assert!(args.contains(&"--directory".into()));
    }

    #[test]
    fn existing_folder_or_pdf_uses_its_own_directory() {
        let temp = tempfile::tempdir().unwrap();
        let folder = temp.path().join("Études with spaces");
        std::fs::create_dir(&folder).unwrap();
        assert_eq!(initial_directory(&folder, temp.path()), folder);
        let file = folder.join("Score.pdf");
        std::fs::write(&file, b"test").unwrap();
        assert_eq!(initial_directory(&file, temp.path()), folder);
        assert_eq!(
            initial_directory(
                Path::new("/not-a-real-library-dir"),
                Path::new("/not-a-home-dir")
            ),
            Path::new("/")
        );
    }
}
