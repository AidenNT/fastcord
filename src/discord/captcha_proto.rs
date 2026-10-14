//! Constantes compartidas entre la app (`captcha.rs`) y la ventana del captcha
//! (`captcha_window.rs`, que en Linux vive en el ejecutable `ecord-captcha`).
#![allow(dead_code)]

/// Bandera con la que `main()` reconoce que este proceso es la ventana del
/// captcha y no la app.
pub const WINDOW_FLAG: &str = "--captcha-window";

/// Cuántos captchas seguidos se le piden a la persona por una misma request
/// antes de darse por vencido (si Discord los rechaza todos, no tiene
/// sentido seguir abriendo ventanas).
pub const MAX_SOLVES_PER_REQUEST: usize = 3;

pub const TOKEN_PREFIX: &str = "ECORD_CAPTCHA_TOKEN:";
pub const ERROR_PREFIX: &str = "ECORD_CAPTCHA_ERROR:";
pub const ENV_SITEKEY: &str = "ECORD_CAPTCHA_SITEKEY";
pub const ENV_RQDATA: &str = "ECORD_CAPTCHA_RQDATA";
pub const ENV_USER_AGENT: &str = "ECORD_CAPTCHA_UA";
pub const HOST_PAGE: &str = "https://discord.com/login";

/// Nombre del ejecutable que muestra la ventana en Linux.
pub const HELPER_NAME: &str = "ecord-captcha";
