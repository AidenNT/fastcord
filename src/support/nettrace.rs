//! Puente entre `reqwest` y `discord_client_rest::netlog` para registrar, en la
//! pestaña Tráfico de las devtools, los pedidos HTTP que hace la app por fuera
//! del REST vendorizado (descargas de imágenes del CDN, build number, login,
//! modelos de audio...).
//!
//! Uso típico (async):
//!
//! ```ignore
//! let trace = nettrace::begin("GET", &url, "fingerprint (build number)");
//! let result = client.get(&url).send().await;
//! nettrace::observe(&trace, &result);
//! ```
//!
//! Con las devtools apagadas, `begin` devuelve un trace vacío y todo lo demás
//! no hace nada. El cuerpo de la respuesta no se guarda desde acá (se sabe
//! solo el tamaño declarado o el que el llamador pase a [`Snap::complete`]).

use discord_client_rest::netlog::{self, HttpTrace};

/// Empieza a registrar un pedido. `via` es la parte de la app que lo hace; el
/// "quién" es lo que declaró el hilo con `netlog::origin` (o su nombre).
pub fn begin(method: &str, url: &str, via: &str) -> HttpTrace {
    netlog::http_begin(method, url, Vec::new(), None, None, via)
}

fn pairs(headers: &reqwest::header::HeaderMap) -> Vec<(String, String)> {
    netlog::header_pairs(headers.iter().map(|(name, value)| (name.as_str(), value.as_bytes())))
}

fn declared_len(headers: &[(String, String)]) -> usize {
    headers
        .iter()
        .find(|(name, _)| name.eq_ignore_ascii_case("content-length"))
        .and_then(|(_, value)| value.trim().parse::<usize>().ok())
        .unwrap_or(0)
}

/// Estado, versión y headers de una respuesta, para cerrar el registro más
/// tarde (cuando ya se leyó el cuerpo y se sabe su tamaño real).
pub struct Snap {
    status: u16,
    version: String,
    headers: Vec<(String, String)>,
}

impl Snap {
    pub fn complete(self, trace: &HttpTrace, len: usize) {
        trace.complete(self.status, &self.version, self.headers, &[], len);
    }
}

pub fn snap(response: &reqwest::Response) -> Snap {
    Snap {
        status: response.status().as_u16(),
        version: format!("{:?}", response.version()),
        headers: pairs(response.headers()),
    }
}

pub fn snap_blocking(response: &reqwest::blocking::Response) -> Snap {
    Snap {
        status: response.status().as_u16(),
        version: format!("{:?}", response.version()),
        headers: pairs(response.headers()),
    }
}

/// Cierra el registro con el resultado de un `send().await`.
pub fn observe(trace: &HttpTrace, result: &Result<reqwest::Response, reqwest::Error>) {
    if !trace.is_active() {
        return;
    }
    match result {
        Ok(response) => {
            let snap = snap(response);
            let len = declared_len(&snap.headers);
            snap.complete(trace, len);
        }
        Err(error) => trace.fail(&error.to_string()),
    }
}

/// Igual que [`observe`] para `reqwest::blocking`.
pub fn observe_blocking(trace: &HttpTrace, result: &Result<reqwest::blocking::Response, reqwest::Error>) {
    if !trace.is_active() {
        return;
    }
    match result {
        Ok(response) => {
            let snap = snap_blocking(response);
            let len = declared_len(&snap.headers);
            snap.complete(trace, len);
        }
        Err(error) => trace.fail(&error.to_string()),
    }
}
