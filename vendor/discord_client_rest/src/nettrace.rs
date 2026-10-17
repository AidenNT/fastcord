//! Puente entre `wreq` y [`crate::netlog`] para registrar (en la pestaña
//! Tráfico de las devtools) los pedidos que no pasan por el camino principal
//! de `RestClient::make_request`: el arranque, el desafío de Cloudflare...
//!
//! Con el registro apagado, `begin` devuelve un trace vacío y el resto no
//! hace nada.

use std::fmt::Display;

use wreq::Response;
use wreq::header::HeaderMap;

use crate::netlog::{self, HttpTrace};

/// Empieza a registrar un pedido. El "quién" es el que declaró el hilo con
/// `netlog::origin` (o el nombre del hilo).
pub(crate) fn begin(method: &str, url: &str, via: &str) -> HttpTrace {
    netlog::http_begin(method, url, Vec::new(), None, None, via)
}

/// Headers en pares de texto.
pub(crate) fn pairs(headers: &HeaderMap) -> Vec<(String, String)> {
    netlog::header_pairs(headers.iter().map(|(name, value)| (name.as_str(), value.as_bytes())))
}

/// Cierra el pedido con lo que se sabe sin leer el cuerpo: estado, headers y
/// el tamaño que declara `Content-Length`.
pub(crate) fn done(trace: &HttpTrace, response: &Response) {
    if !trace.is_active() {
        return;
    }
    let headers = pairs(response.headers());
    let length = headers
        .iter()
        .find(|(name, _)| name.eq_ignore_ascii_case("content-length"))
        .and_then(|(_, value)| value.trim().parse::<usize>().ok())
        .unwrap_or(0);
    trace.complete(
        response.status().as_u16(),
        &format!("{:?}", response.version()),
        headers,
        &[],
        length,
    );
}

/// Registra el resultado de un `send().await` (éxito o error de red).
pub(crate) fn observe<E: Display>(trace: &HttpTrace, result: &Result<Response, E>) {
    match result {
        Ok(response) => done(trace, response),
        Err(error) => trace.fail(&error.to_string()),
    }
}
