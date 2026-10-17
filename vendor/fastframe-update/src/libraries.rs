//! The libraries (on Windows, DLLs) that travel beside an app and have to
//! change with it: [`UpdateConfig::libraries`].
//!
//! Only the executable used to be replaced. An app that links FFmpeg or ONNX
//! Runtime dynamically then ran a new executable against the old DLLs, which
//! crashes on start the day a DLL's version changes. The update now carries
//! them too:
//!
//! * **download** ([`stage`]): the archive's libraries are unpacked into the
//!   staging folder's `libs/`, with their SHA-256 in `libs.txt`, and the new
//!   executable is probed with *those* libraries beside it
//!   ([`probe_version`]), not with whatever happens to be in the app's folder.
//!   A library that cannot be unpacked fails the whole download: an update
//!   with a half-replaced set is worse than none.
//! * **helper** ([`snapshot`], [`install`], [`restore`]): once the app has
//!   exited, the libraries it has now are copied to `libs-previous/`, the new
//!   ones replace them (each through a temporary file, so a file is either the
//!   old or the new one), and a failed replacement or a new app that does not
//!   start puts the old ones back, together with the old executable.
//!
//! The Windows setup program installs its own files, so for it only
//! [`snapshot`] and [`restore`] apply: they let a rollback undo what the
//! installer overwrote.

use std::collections::HashSet;
use std::fs;
use std::path::Path;
use std::time::Duration;

use anyhow::{Context, Result, ensure};

use crate::detect::Platform;
use crate::download::check_version;
use crate::host::Host;
use crate::release;
use crate::stage::{
    self, LIBRARIES, LIBRARIES_MANIFEST, LIBRARIES_PREVIOUS, Staged,
};
use crate::UpdateConfig;

/// More than this many libraries in one archive is not an app's own set.
const MAX_LIBRARIES: usize = 256;
/// A library another process still holds (an antivirus scanning the file the
/// app just released) frees up within a moment: this many tries, 200 ms apart.
const REPLACE_TRIES: u32 = 15;

/// A plain file name: no folders, no drive, no leading dot.
fn plain_name(name: &str) -> bool {
    !name.is_empty()
        && !name.starts_with('.')
        && !name.contains(['/', '\\', ':', '\0'])
        && name.len() <= 128
}

/// Whether `name` has one of the configured library extensions.
pub(crate) fn is_library(config: &UpdateConfig, name: &str) -> bool {
    Path::new(name)
        .extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| {
            config
                .libraries
                .iter()
                .any(|wanted| wanted.eq_ignore_ascii_case(extension))
        })
}

/// Unpacks the archive's libraries into `<directory>/libs/` and records their
/// digests. The archive is flat inside `stem/`: anything in a subfolder is
/// not a library of the app. Returns the names staged (none when the app
/// configured no libraries, or the archive carries none).
pub(crate) fn stage(
    config: &UpdateConfig,
    host: &dyn Host,
    platform: Platform,
    archive: &Path,
    stem: &str,
    directory: &Path,
) -> Result<Vec<String>> {
    if config.libraries.is_empty() {
        return Ok(Vec::new());
    }
    let executable = release::portable_executable(config, platform);
    let helper = release::helper_executable(config, platform);
    let prefix = format!("{stem}/");
    let mut names: Vec<String> = host
        .list_entries(archive)?
        .into_iter()
        .filter_map(|entry| entry.strip_prefix(&prefix).map(str::to_owned))
        .filter(|name| {
            plain_name(name)
                && is_library(config, name)
                && !name.eq_ignore_ascii_case(&executable)
                && helper
                    .as_deref()
                    .is_none_or(|helper| !name.eq_ignore_ascii_case(helper))
        })
        .collect();
    names.sort();
    names.dedup();
    ensure!(
        names.len() <= MAX_LIBRARIES,
        "The update carries too many libraries"
    );
    if names.is_empty() {
        return Ok(names);
    }
    let folder = directory.join(LIBRARIES);
    fs::create_dir(&folder).context("Cannot create the staged libraries folder")?;
    let mut manifest = String::new();
    for name in &names {
        let destination = folder.join(name);
        host.extract(archive, &format!("{prefix}{name}"), &destination)
            .with_context(|| format!("Cannot unpack {name} from the update"))?;
        manifest.push_str(&format!("{}  {name}\n", stage::hash(&destination)?));
    }
    fs::write(directory.join(LIBRARIES_MANIFEST), manifest)
        .context("Cannot record the staged libraries")?;
    Ok(names)
}

/// Runs `<executable> --version` the way the app will run: from a copy of the
/// staged executable placed among the staged libraries, so Windows resolves
/// the new DLLs and not the old ones in the app's folder.
pub(crate) fn probe_version(
    config: &UpdateConfig,
    host: &dyn Host,
    payload: &Path,
    directory: &Path,
    version: &str,
) -> Result<()> {
    let name = payload
        .file_name()
        .context("The staged executable has no name")?;
    let probe = directory.join(LIBRARIES).join(name);
    fs::copy(payload, &probe).context("Cannot prepare the startup check")?;
    let result = check_version(config, host, &probe, version);
    let _ = fs::remove_file(&probe);
    result
}

/// The staged libraries and their digests, or none if the update has none.
fn staged_libraries(staged: &Staged) -> Result<Vec<(String, String)>> {
    let manifest = staged.file(LIBRARIES_MANIFEST);
    if !manifest.is_file() {
        return Ok(Vec::new());
    }
    let text = fs::read_to_string(&manifest).context("Cannot read the staged libraries")?;
    text.lines()
        .filter(|line| !line.is_empty())
        .map(|line| {
            let (digest, name) = line
                .split_once("  ")
                .context("Invalid staged libraries list")?;
            ensure!(
                digest.len() == 64
                    && digest.bytes().all(|byte| byte.is_ascii_hexdigit())
                    && plain_name(name),
                "Invalid staged libraries list"
            );
            Ok((name.to_owned(), digest.to_ascii_lowercase()))
        })
        .collect()
}

/// The folder the app runs from.
fn app_folder(staged: &Staged) -> Result<&Path> {
    staged
        .installation
        .executable
        .parent()
        .context("Missing installation directory")
}

/// `rename` over `target`, trying again for a moment when Windows reports the
/// file in use.
fn replace_file(temporary: &Path, target: &Path) -> Result<()> {
    let mut last = None;
    for attempt in 0..REPLACE_TRIES {
        match fs::rename(temporary, target) {
            Ok(()) => return Ok(()),
            Err(error) => last = Some(error),
        }
        if attempt + 1 < REPLACE_TRIES {
            std::thread::sleep(Duration::from_millis(200));
        }
    }
    let _ = fs::remove_file(temporary);
    match last {
        Some(error) => Err(error.into()),
        None => Ok(()),
    }
}

/// Copies the libraries the app has now to `libs-previous/`, so a rollback can
/// restore them. Called once the app has exited, before anything is replaced.
/// A leftover folder means this update was already tried.
pub(crate) fn snapshot(config: &UpdateConfig, staged: &Staged) -> Result<()> {
    if config.libraries.is_empty() {
        return Ok(());
    }
    let backup = staged.file(LIBRARIES_PREVIOUS);
    fs::create_dir(&backup).context("This update's libraries were already backed up")?;
    for entry in fs::read_dir(app_folder(staged)?)? {
        let entry = entry?;
        let name = entry.file_name().to_string_lossy().into_owned();
        if entry.file_type()?.is_file() && plain_name(&name) && is_library(config, &name) {
            fs::copy(entry.path(), backup.join(&name))
                .with_context(|| format!("Cannot back up {name}"))?;
        }
    }
    Ok(())
}

/// Puts the staged libraries in the app's folder, replacing the ones there.
/// Checks every digest first, so nothing changes if a staged file was altered.
/// Returns how many it installed.
pub(crate) fn install(staged: &Staged) -> Result<usize> {
    let libraries = staged_libraries(staged)?;
    let source = staged.file(LIBRARIES);
    for (name, digest) in &libraries {
        ensure!(
            stage::hash(&source.join(name))? == *digest,
            "The staged library {name} changed"
        );
    }
    let app = app_folder(staged)?;
    for (name, _) in &libraries {
        let temporary = app.join(format!("{name}.update-new"));
        let _ = fs::remove_file(&temporary);
        fs::copy(source.join(name), &temporary)
            .with_context(|| format!("Cannot copy {name} into the app's folder"))?;
        replace_file(&temporary, &app.join(name))
            .with_context(|| format!("Cannot replace {name}: another program is using it"))?;
    }
    Ok(libraries.len())
}

/// Undoes [`install`] and whatever the setup program overwrote: puts back the
/// libraries [`snapshot`] copied, and removes the ones the update added.
/// Does nothing when no snapshot was taken (nothing was touched). Tries every
/// file and reports the first failure.
pub(crate) fn restore(config: &UpdateConfig, staged: &Staged) -> Result<()> {
    let backup = staged.file(LIBRARIES_PREVIOUS);
    if config.libraries.is_empty() || !backup.is_dir() {
        return Ok(());
    }
    let app = app_folder(staged)?;
    let mut first_error: Option<anyhow::Error> = None;
    let mut keep = |result: Result<()>| {
        if let Err(error) = result {
            let _ = first_error.get_or_insert(error);
        }
    };
    let mut previous: HashSet<String> = HashSet::new();
    for entry in fs::read_dir(&backup)? {
        let entry = entry?;
        let name = entry.file_name().to_string_lossy().into_owned();
        if !plain_name(&name) {
            continue;
        }
        previous.insert(name.to_ascii_lowercase());
        let temporary = app.join(format!("{name}.update-old"));
        keep(
            fs::copy(entry.path(), &temporary)
                .map(|_| ())
                .map_err(anyhow::Error::from)
                .and_then(|()| replace_file(&temporary, &app.join(&name)))
                .with_context(|| format!("Cannot restore {name}")),
        );
    }
    // What the update added did not exist before.
    for (name, _) in staged_libraries(staged).unwrap_or_default() {
        if !previous.contains(&name.to_ascii_lowercase()) {
            let _ = fs::remove_file(app.join(&name));
        }
    }
    match first_error {
        Some(error) => Err(error),
        None => Ok(()),
    }
}

/// Deletes the backup once the new version has started.
pub(crate) fn discard_backup(staged: &Staged) {
    let _ = fs::remove_dir_all(staged.file(LIBRARIES_PREVIOUS));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::detect::{Installation, Kind};
    use crate::tests::ZAPFAST;

    const DLLS: UpdateConfig = UpdateConfig {
        libraries: &["dll"],
        ..ZAPFAST
    };

    /// An installed app with `old` libraries and a staging folder holding
    /// `new` ones.
    fn staged(root: &Path, old: &[(&str, &[u8])], new: &[(&str, &[u8])]) -> Staged {
        let executable = root.join("zapfast");
        fs::write(&executable, b"old executable").unwrap();
        for (name, contents) in old {
            fs::write(root.join(name), contents).unwrap();
        }
        let installation = Installation {
            executable: executable.canonicalize().unwrap(),
            kind: Kind::Portable,
        };
        let directory = stage::staging(&DLLS, &installation).unwrap();
        let payload = directory.join("zapfast");
        fs::write(&payload, b"new executable").unwrap();
        if !new.is_empty() {
            fs::create_dir(directory.join(LIBRARIES)).unwrap();
            let mut manifest = String::new();
            for (name, contents) in new {
                let path = directory.join(LIBRARIES).join(name);
                fs::write(&path, contents).unwrap();
                manifest.push_str(&format!("{}  {name}\n", stage::hash(&path).unwrap()));
            }
            fs::write(directory.join(LIBRARIES_MANIFEST), manifest).unwrap();
        }
        Staged {
            sha256: stage::hash(&payload).unwrap(),
            installation,
            directory,
            payload,
            version: "0.17.0".into(),
        }
    }

    #[test]
    fn only_plain_names_with_a_configured_extension_are_libraries() {
        assert!(plain_name("avcodec-61.dll"));
        for bad in ["", ".hidden.dll", "sub/avcodec.dll", "..\\avcodec.dll", "C:evil.dll"] {
            assert!(!plain_name(bad), "{bad}");
        }
        assert!(is_library(&DLLS, "AVCODEC-61.DLL"));
        assert!(!is_library(&DLLS, "zapfast.exe"));
        assert!(!is_library(&ZAPFAST, "avcodec-61.dll"));
    }

    #[test]
    fn staging_unpacks_only_flat_libraries_of_the_archive() {
        let root = tempfile::tempdir().unwrap();
        let host = crate::testing::FakeHost::default()
            .with_archive_entry("zapfast-v0.17.0-t/zapfast.exe", b"exe")
            .with_archive_entry("zapfast-v0.17.0-t/avcodec-62.dll", b"codec")
            .with_archive_entry("zapfast-v0.17.0-t/onnxruntime.DLL", b"onnx")
            .with_archive_entry("zapfast-v0.17.0-t/sub/evil.dll", b"evil")
            .with_archive_entry("zapfast-v0.17.0-t/readme.txt", b"text")
            .with_archive_entry("other-v0.17.0-t/other.dll", b"other");
        let names = stage(
            &DLLS,
            &host,
            Platform::Windows,
            Path::new("archive.zip"),
            "zapfast-v0.17.0-t",
            root.path(),
        )
        .unwrap();
        assert_eq!(names, ["avcodec-62.dll", "onnxruntime.DLL"]);
        assert_eq!(
            fs::read(root.path().join(LIBRARIES).join("avcodec-62.dll")).unwrap(),
            b"codec"
        );
        assert!(!root.path().join(LIBRARIES).join("evil.dll").exists());
        let manifest = fs::read_to_string(root.path().join(LIBRARIES_MANIFEST)).unwrap();
        assert_eq!(manifest.lines().count(), 2);
    }

    #[test]
    fn an_app_without_libraries_stages_none() {
        let root = tempfile::tempdir().unwrap();
        let host = crate::testing::FakeHost::default()
            .with_archive_entry("zapfast-v0.17.0-t/avcodec-62.dll", b"codec");
        let names = stage(
            &ZAPFAST,
            &host,
            Platform::Windows,
            Path::new("archive.zip"),
            "zapfast-v0.17.0-t",
            root.path(),
        )
        .unwrap();
        assert!(names.is_empty());
        assert!(!root.path().join(LIBRARIES).exists());
    }

    #[test]
    fn installing_replaces_adds_and_a_restore_undoes_both() {
        let root = tempfile::tempdir().unwrap();
        let staged = staged(
            root.path(),
            &[("avcodec-61.dll", b"old codec"), ("keep.dll", b"keep")],
            &[("avcodec-61.dll", b"new codec"), ("extra.dll", b"extra")],
        );
        snapshot(&DLLS, &staged).unwrap();
        assert_eq!(install(&staged).unwrap(), 2);
        assert_eq!(fs::read(root.path().join("avcodec-61.dll")).unwrap(), b"new codec");
        assert_eq!(fs::read(root.path().join("extra.dll")).unwrap(), b"extra");
        assert_eq!(fs::read(root.path().join("keep.dll")).unwrap(), b"keep");

        restore(&DLLS, &staged).unwrap();
        assert_eq!(fs::read(root.path().join("avcodec-61.dll")).unwrap(), b"old codec");
        assert!(!root.path().join("extra.dll").exists());
        assert_eq!(fs::read(root.path().join("keep.dll")).unwrap(), b"keep");
        assert!(!root.path().join("avcodec-61.dll.update-new").exists());
    }

    #[test]
    fn a_changed_staged_library_installs_nothing() {
        let root = tempfile::tempdir().unwrap();
        let staged = staged(
            root.path(),
            &[("a.dll", b"old a")],
            &[("a.dll", b"new a"), ("b.dll", b"new b")],
        );
        fs::write(staged.file(LIBRARIES).join("b.dll"), b"tampered").unwrap();
        snapshot(&DLLS, &staged).unwrap();
        assert!(install(&staged).is_err());
        assert_eq!(fs::read(root.path().join("a.dll")).unwrap(), b"old a");
        assert!(!root.path().join("b.dll").exists());
    }

    #[test]
    fn a_restore_without_a_snapshot_touches_nothing() {
        let root = tempfile::tempdir().unwrap();
        let staged = staged(root.path(), &[("a.dll", b"old a")], &[("a.dll", b"new a")]);
        restore(&DLLS, &staged).unwrap();
        assert_eq!(fs::read(root.path().join("a.dll")).unwrap(), b"old a");
    }

    #[test]
    fn a_second_snapshot_of_the_same_update_is_refused() {
        let root = tempfile::tempdir().unwrap();
        let staged = staged(root.path(), &[("a.dll", b"old a")], &[]);
        snapshot(&DLLS, &staged).unwrap();
        assert!(snapshot(&DLLS, &staged).is_err());
    }
}
