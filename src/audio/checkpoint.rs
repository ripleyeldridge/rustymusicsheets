//! Verify untrusted helper output before committing content-addressed state.
use super::plugins::{hex, CheckpointPart, Source, StateFile, MAX_SESSION_STATE, MAX_STATE};
use anyhow::{ensure, Context, Result};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs::{self, File},
    io::{Read, Write},
    path::Path,
};

#[cfg(unix)]
fn private_metadata(path: &Path, directory: bool) -> Result<()> {
    use std::os::unix::fs::MetadataExt;
    let metadata = fs::symlink_metadata(path)?;
    ensure!(
        !metadata.file_type().is_symlink()
            && if directory {
                metadata.is_dir()
            } else {
                metadata.is_file()
            }
            && metadata.uid() == unsafe { libc::geteuid() }
            && metadata.mode() & 0o7777 == if directory { 0o700 } else { 0o600 },
        "State path {} is not private, owned and regular (mode {:o})",
        path.display(),
        metadata.mode() & 0o7777
    );
    Ok(())
}

#[cfg(not(unix))]
fn private_metadata(path: &Path, directory: bool) -> Result<()> {
    let metadata = fs::symlink_metadata(path)?;
    ensure!(
        !metadata.file_type().is_symlink() && metadata.is_dir() == directory,
        "Invalid state file"
    );
    Ok(())
}

fn read_verified(file: &StateFile) -> Result<Vec<u8>> {
    read_verified_at(file, None)
}

fn read_verified_at(file: &StateFile, directory: Option<&File>) -> Result<Vec<u8>> {
    ensure!(
        file.bytes <= MAX_STATE && hex(&file.sha256, 64, false),
        "Invalid state size/hash"
    );
    private_metadata(&file.path, false)?;
    // O_NOFOLLOW closes the final-symlink swap between lstat and open.
    let mut options = fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC);
    }
    let input = if let Some(directory) = directory {
        use std::os::{
            fd::{AsRawFd, FromRawFd},
            unix::ffi::OsStrExt,
        };
        let name = std::ffi::CString::new(
            file.path
                .file_name()
                .context("Missing state name")?
                .as_bytes(),
        )?;
        // SAFETY: directory is a live O_DIRECTORY fd, name is NUL-terminated,
        // and a successful returned descriptor is transferred exactly once.
        let fd = unsafe {
            libc::openat(
                directory.as_raw_fd(),
                name.as_ptr(),
                libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC,
            )
        };
        ensure!(
            fd >= 0,
            "Cannot open contained checkpoint file: {}",
            std::io::Error::last_os_error()
        );
        unsafe { File::from_raw_fd(fd) }
    } else {
        options.open(&file.path)?
    };
    use std::os::unix::fs::MetadataExt;
    let metadata = input.metadata()?;
    ensure!(
        metadata.is_file()
            && metadata.uid() == unsafe { libc::geteuid() }
            && metadata.mode() & 0o7777 == 0o600
            && metadata.nlink() == 1
            && metadata.len() == file.bytes,
        "State changed or is not a private owned regular single-link file"
    );
    let mut bytes = Vec::new();
    input.take(MAX_STATE + 1).read_to_end(&mut bytes)?;
    ensure!(
        bytes.len() as u64 == file.bytes && format!("{:x}", Sha256::digest(&bytes)) == file.sha256,
        "State digest/size mismatch"
    );
    Ok(bytes)
}

pub fn verify_restore(sources: &BTreeMap<String, Source>) -> Result<()> {
    let mut total = 0u64;
    for source in sources.values() {
        source.validate()?;
        if let Source::Vst3 {
            component_state,
            controller_state,
            ..
        } = source
        {
            for state in [component_state, controller_state].into_iter().flatten() {
                total = total
                    .checked_add(state.bytes)
                    .context("State size overflow")?;
                ensure!(total <= MAX_SESSION_STATE, "Session state exceeds 256 MiB");
                read_verified(state)?;
            }
        }
    }
    Ok(())
}

pub fn persist(
    staging: &Path,
    destination: &Path,
    sources: &BTreeMap<String, Source>,
    parts: &mut [CheckpointPart],
) -> Result<()> {
    private_metadata(staging, true)?;
    use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
    let staging_fd = fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(staging)?;
    let metadata = staging_fd.metadata()?;
    ensure!(
        metadata.is_dir()
            && metadata.uid() == unsafe { libc::geteuid() }
            && metadata.mode() & 0o7777 == 0o700,
        "Staging directory changed"
    );
    ensure!(
        parts.len() == sources.values().filter(|s| s.is_plugin()).count(),
        "Incomplete checkpoint"
    );
    let mut seen = BTreeSet::new();
    let mut total = 0u64;
    let mut verified = Vec::new();
    for (index, part) in parts.iter().enumerate() {
        ensure!(seen.insert(&part.part_id), "Duplicate checkpoint part");
        let Some(Source::Vst3 { class_id, .. }) = sources.get(&part.part_id) else {
            anyhow::bail!("Checkpoint names an unknown plugin instance");
        };
        ensure!(&part.class_id == class_id, "Checkpoint class mismatch");
        let mut part_bytes = 0u64;
        for (controller, file) in [
            (false, &part.component_state),
            (true, &part.controller_state),
        ] {
            if let Some(file) = file {
                ensure!(
                    file.path.is_absolute()
                        && file.path.parent() == Some(staging)
                        && file.path.file_name().is_some()
                        && !file
                            .path
                            .components()
                            .any(|c| matches!(c, std::path::Component::ParentDir)),
                    "Checkpoint escapes fresh staging directory"
                );
                let bytes = read_verified_at(file, Some(&staging_fd))?;
                part_bytes = part_bytes
                    .checked_add(file.bytes)
                    .context("State size overflow")?;
                total = total
                    .checked_add(file.bytes)
                    .context("State size overflow")?;
                ensure!(
                    part_bytes <= MAX_STATE && total <= MAX_SESSION_STATE,
                    "Checkpoint exceeds size limit"
                );
                verified.push((index, controller, file.sha256.clone(), bytes));
            }
        }
    }
    // Nothing is referenced by a manifest until every output has been verified.
    use std::os::unix::fs::DirBuilderExt;
    fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(destination)?;
    private_metadata(destination, true)?;
    for (index, controller, hash, bytes) in verified {
        let target = destination.join(format!("{hash}.state"));
        let state = StateFile {
            path: target.clone(),
            sha256: hash,
            bytes: bytes.len() as u64,
        };
        if target.exists() {
            read_verified(&state)?;
        } else {
            let mut output = tempfile::NamedTempFile::new_in(destination)?;
            output.write_all(&bytes)?;
            output.as_file().sync_all()?;
            match output.persist_noclobber(&target) {
                Ok(_) => {}
                Err(error) if error.error.kind() == std::io::ErrorKind::AlreadyExists => {
                    read_verified(&state)?;
                }
                Err(error) => return Err(error.into()),
            }
        }
        if controller {
            parts[index].controller_state = Some(state);
        } else {
            parts[index].component_state = Some(state);
        }
    }
    File::open(destination)?.sync_all()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    fn file(root: &Path, name: &str) -> StateFile {
        let path = root.join(name);
        fs::write(&path, name.as_bytes()).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
        StateFile {
            path,
            sha256: format!("{:x}", Sha256::digest(name.as_bytes())),
            bytes: name.len() as u64,
        }
    }
    #[test]
    fn checkpoint_verifies_containment_modes_hashes_and_keeps_separate_instances() {
        let staging = tempfile::tempdir().unwrap();
        let persistent = tempfile::tempdir().unwrap();
        fs::set_permissions(staging.path(), fs::Permissions::from_mode(0o700)).unwrap();
        fs::set_permissions(persistent.path(), fs::Permissions::from_mode(0o700)).unwrap();
        let class = "A".repeat(32);
        let sources = ["P1", "P2"]
            .map(|id| {
                (
                    id.into(),
                    Source::Vst3 {
                        path: Path::new("/test.vst3").into(),
                        class_id: class.clone(),
                        module_sha256: "a".repeat(64),
                        gain: 0.5,
                        component_state: None,
                        controller_state: None,
                    },
                )
            })
            .into_iter()
            .collect();
        let mut parts: Vec<_> = ["P1", "P2"]
            .map(|id| CheckpointPart {
                part_id: id.into(),
                class_id: class.clone(),
                component_state: Some(file(staging.path(), &format!("{id}.component"))),
                controller_state: Some(file(staging.path(), &format!("{id}.controller"))),
            })
            .into();
        persist(staging.path(), persistent.path(), &sources, &mut parts).unwrap();
        assert_ne!(parts[0].component_state, parts[1].component_state);
        assert_ne!(parts[0].component_state, parts[0].controller_state);
        let saved = parts[0].component_state.as_ref().unwrap().clone();
        for bad in 0..8 {
            let stage = tempfile::tempdir().unwrap();
            fs::set_permissions(stage.path(), fs::Permissions::from_mode(0o700)).unwrap();
            let mut part = CheckpointPart {
                part_id: "P1".into(),
                class_id: class.clone(),
                component_state: Some(file(stage.path(), "component")),
                controller_state: None,
            };
            let state = part.component_state.as_mut().unwrap();
            match bad {
                0 => state.path = saved.path.clone(),
                1 => state.sha256 = "0".repeat(64),
                2 => fs::set_permissions(&state.path, fs::Permissions::from_mode(0o644)).unwrap(),
                3 => {
                    fs::remove_file(&state.path).unwrap();
                    std::os::unix::fs::symlink(&saved.path, &state.path).unwrap();
                }
                4 => state.bytes = MAX_STATE + 1,
                5 => part.class_id = "B".repeat(32),
                6 => fs::hard_link(&state.path, stage.path().join("hardlink")).unwrap(),
                _ => {
                    use std::os::unix::ffi::OsStrExt;
                    fs::remove_file(&state.path).unwrap();
                    let name = std::ffi::CString::new(state.path.as_os_str().as_bytes()).unwrap();
                    assert_eq!(unsafe { libc::mkfifo(name.as_ptr(), 0o600) }, 0);
                }
            }
            let one = BTreeMap::from([("P1".into(), sources["P1"].clone())]);
            assert!(persist(stage.path(), persistent.path(), &one, &mut [part]).is_err());
            assert!(saved.path.exists(), "Last valid checkpoint must remain");
        }
    }
}
