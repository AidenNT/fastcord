//! Escritura de archivos que solo debería poder leer el usuario (cookies y
//! ids de sesión). En Unix los deja en `0600` / `0700`; en Windows los
//! permisos por defecto de la carpeta del usuario ya son privados, así que
//! ahí no se toca nada.

use std::{fs, io, path::Path};

/// Deja la carpeta accesible solo para el usuario (`0700`).
#[cfg(unix)]
pub fn set_private_dir_permissions(path: &Path) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(0o700))
}

#[cfg(not(unix))]
pub fn set_private_dir_permissions(path: &Path) -> io::Result<()> {
    let _ = path;
    Ok(())
}

/// Escribe (o pisa) `path` con `content`, creándolo con `0600`. Si el
/// archivo ya existía con otros permisos también se corrigen.
#[cfg(unix)]
pub fn write_private_file(path: &Path, content: impl AsRef<[u8]>) -> io::Result<()> {
    use std::io::Write;
    use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};

    let mut file = fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(path)?;
    file.write_all(content.as_ref())?;
    file.flush()?;
    fs::set_permissions(path, fs::Permissions::from_mode(0o600))
}

#[cfg(not(unix))]
pub fn write_private_file(path: &Path, content: impl AsRef<[u8]>) -> io::Result<()> {
    fs::write(path, content)
}
