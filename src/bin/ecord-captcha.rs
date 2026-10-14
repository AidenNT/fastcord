//! `ecord-captcha`: ventana con el hCaptcha de Discord (solo Linux).
//!
//! Es el único ejecutable que enlaza WebKitGTK y GTK. `ecord` lo lanza cuando
//! Discord pide un captcha; así el cliente arranca aunque el sistema no tenga
//! esas bibliotecas (portable sin gestor de paquetes). Si faltan, este proceso
//! ni siquiera inicia y `ecord` le avisa a la persona qué instalar.
//!
//! Protocolo: recibe sitekey/rqdata/user-agent por variables de entorno
//! (`ECORD_CAPTCHA_*`) y devuelve el token por stdout en una línea
//! `ECORD_CAPTCHA_TOKEN:<token>` (ver `src/discord/captcha_proto.rs`).

#[cfg(target_os = "linux")]
#[path = "../discord/captcha_proto.rs"]
mod proto;
#[cfg(target_os = "linux")]
#[path = "../discord/captcha_window.rs"]
mod window;

#[cfg(target_os = "linux")]
fn main() {
    window::run_window();
}

#[cfg(not(target_os = "linux"))]
fn main() {
    eprintln!("ecord-captcha solo se usa en Linux; en este sistema la ventana va dentro de ecord.");
    std::process::exit(1);
}
