//! `ecord-updater`: el ayudante que instala las actualizaciones de eCord.
//!
//! `ecord` baja y verifica la versión nueva y, al reiniciar, copia ESTE
//! ejecutable a la carpeta temporal de la actualización y lo lanza con
//! `--apply-update <trabajo>`. Esa copia espera a que `ecord` se cierre,
//! reemplaza el ejecutable (guardando un respaldo), relanza `ecord` y, si la
//! versión nueva no arranca en un minuto, restaura la anterior. Como no
//! enlaza FFmpeg ni nada de la app, no hace falta copiar sus DLL a ninguna parte
//! (antes, en Windows, el ayudante era una copia entera de `ecord` con todas las
//! DLL) y puede ejecutarse mientras `ecord` está reemplazándose.
//!
//! Tras una actualización con éxito, el ayudante instala junto a `ecord` el
//! `ecord-updater` nuevo que traía el paquete, para que la próxima vez corra
//! la versión actual.
//!
//! También se ocupa de las DLL (FFmpeg de las transmisiones, ONNX Runtime): en
//! el zip portable reemplaza las del paquete junto con el ejecutable y, si la
//! versión nueva no arranca, devuelve las anteriores (ver `UPDATES.libraries`).
//! Con el `-setup.exe` (NSIS) lo ejecuta en silencio (`/S /D=<carpeta>`), y el
//! instalador pone sus propias DLL; el ayudante solo guarda una copia de las
//! actuales para poder volver atrás.
//!
//! Ojo: el ayudante que instala una actualización es siempre el que ya estaba
//! instalado, así que estos cambios se notan recién en la actualización
//! SIGUIENTE a la que los trae.
//!
//! Solo hace eso: ejecutado a mano imprime cómo se usa y sale.

// En release (Windows) no abre consola negra.
#![cfg_attr(all(not(debug_assertions), windows), windows_subsystem = "windows")]
// Comparte la configuración con `ecord`; de ella acá solo se usa `UPDATES`.
#[allow(dead_code)]
#[path = "../update_config.rs"]
mod update_config;

fn main() {
    // `ecord --apply-update` lo lanza `ecord` con esa forma exacta; también
    // responde `--version` igual que `ecord`, porque la actualización anterior
    // lo ejecuta para comprobar el paquete antes de instalarlo.
    if std::env::args_os().skip(1).any(|arg| arg == "--version") {
        #[allow(clippy::print_stdout)]
        {
            println!("{} {}", update_config::UPDATES.slug, update_config::UPDATES.current_version);
        }
        return;
    }
    // Si fue invocado como ayudante, instala y sale sin volver.
    fastframe_update::apply_if_requested(&update_config::UPDATES);
    #[allow(clippy::print_stderr)]
    {
        eprintln!(
            "ecord-updater {}: instala las actualizaciones de eCord; lo lanza ecord, no se usa a mano.",
            update_config::UPDATES.current_version
        );
    }
    std::process::exit(2);
}
