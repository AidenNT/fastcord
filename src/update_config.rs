//! Configuración del actualizador, compartida por `ecord` (src/updater.rs) y por
//! `ecord-updater` (src/bin/ecord-updater.rs, vía `#[path]`): los dos tienen que
//! describir la instalación exactamente igual (nombres de archivo, clave pública,
//! versión). No usa nada de `crate::` a propósito, para poder incluirse en el
//! otro ejecutable.

use fastframe_update::{InstallerKind, MacConfig, UpdateConfig};

/// Repositorio de GitHub (`dueño/nombre`) donde se publican las versiones.
///
/// TODO(antes de publicar): poné el tuyo. Mientras siga con este valor de
/// ejemplo, las actualizaciones quedan desactivadas (ver `UpdateConfig::validate`
/// y el test `la_configuracion_es_valida` de src/updater.rs).
pub const REPOSITORY: &str = "AidenNT/fastcord";

/// Nombre (sin extensión) del ejecutable que instala las actualizaciones. Va
/// junto a `ecord` en el zip portable, el instalador y el .tar.gz.
pub const HELPER: &str = "ecord-updater";

/// Extensiones de las bibliotecas que viajan junto a `ecord` y se actualizan con
/// él: en Windows, las DLL de FFmpeg (transmisiones, códecs) y `onnxruntime.dll`
/// (DeepFilterNet). El updater las baja y verifica con el ejecutable, prueba el
/// ejecutable nuevo CON ellas, las reemplaza mientras `ecord` está cerrado y las
/// restaura junto con el ejecutable si la versión nueva no arranca. En Linux y
/// macOS las bibliotecas no van como archivos sueltos junto al ejecutable.
#[cfg(windows)]
const LIBRARIES: &[&str] = &["dll"];
#[cfg(not(windows))]
const LIBRARIES: &[&str] = &[];

/// Cómo se describe ecord al updater. El nombre de comando (`ecord`) define
/// los nombres de los archivos de cada versión (`ecord-v0.1.0-x86_64-unknown-linux-gnu.tar.gz`),
/// el archivo marcador (`ecord-portable.txt`) y lo que imprime `--version`.
pub const UPDATES: UpdateConfig = UpdateConfig {
    macos: MacConfig {
        // El mismo `identifier` de `[package.metadata.packager]` en Cargo.toml.
        bundle_ids: &["dev.ecord.client"],
        executable_names: &[],
        legacy_bundle_names: &[],
    },
    // Clave PÚBLICA Ed25519 (64 caracteres hexadecimales). Cada versión tiene
    // que publicar `checksums.txt` y `checksums.txt.sig` firmado con la clave
    // privada correspondiente; sin firma válida no se descarga nada.
    publisher_key: Some(include_str!("../assets/update-public-key.hex")),
    // Quien instala es `ecord-updater`, un ejecutable chico aparte (sin las DLL
    // de FFmpeg ni nada de la app), no una copia de `ecord`.
    helper_executable: Some(HELPER),
    // El `-setup.exe` lo arma `cargo packager --formats nsis` (NSIS), que no
    // entiende los `/VERYSILENT` de Inno Setup y abría el asistente: se le
    // pasa `/S /D=<carpeta>`.
    installer: InstallerKind::Nsis,
    libraries: LIBRARIES,
    ..UpdateConfig::new(REPOSITORY, "eCord", "ecord", env!("CARGO_PKG_VERSION"))
};
