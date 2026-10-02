use std::error::Error;
pub mod api;
mod bootstrap;
pub mod captcha;
pub mod clearance;
pub mod image;
pub mod mfa;
pub mod rate_limit;
mod response;
pub mod rest;
pub mod sessionless;
pub mod structs;
pub mod super_prop;

/// Variable de entorno que desactiva la verificación de certificados TLS
/// (solo para depurar con un proxy de interceptación).
pub const ENV_INSECURE_TLS: &str = "ECORD_INSECURE_TLS";

/// ¿Está activo `ECORD_INSECURE_TLS=1`? Es la MISMA comprobación que usa
/// `bootstrap::build_emulated_client` para apagar la verificación, así que
/// el aviso de seguridad de la UI siempre coincide con lo que realmente pasa.
pub fn insecure_tls_enabled() -> bool {
    std::env::var(ENV_INSECURE_TLS).as_deref() == Ok("1")
}

type BoxedError = Box<dyn Error + Send + Sync>;
type BoxedResult<T> = Result<T, BoxedError>;

const MAX_ICON_SIZE: usize = 10 * 1024 * 1024;
