//! Grafía (mayúsculas) y orden de los headers en el cable.
//!
//! Con HTTP/1.1 los nombres de header viajan tal cual se escriben. `wreq`
//! los guarda en minúscula (`http::HeaderName` normaliza) y, si nadie le dice
//! lo contrario, así los manda: `x-super-properties`, `authorization`...
//! Un navegador real nunca hace eso en HTTP/1.1, y se ve en HTTP Toolkit.
//!
//! `OrigHeaderMap` fija solo CÓMO se escribe cada nombre y en qué orden; los
//! valores siguen siendo los de la emulación o los que se agreguen aparte. Los
//! nombres de la lista que el request no lleva se ignoran, así que alcanza
//! con una lista única que cubra todos los requests de la crate (incluidos
//! los headers que agrega la emulación de Chrome: `Host`, `Accept`, `Cookie`...).
//!
//! Con HTTP/2 los nombres van siempre en minúscula por protocolo y esto no
//! cambia nada.
//!
//! Si alguna vez hace falta cambiar la grafía de un header (por ejemplo
//! `Sec-CH-UA` por `sec-ch-ua`, como lo manda Chrome de verdad), se hace acá y
//! alcanza a todo el REST.

use wreq::header::OrigHeaderMap;

/// Todos los nombres que mandamos o que agrega la emulación, en el orden de
/// Chrome en el cable.
const HEADER_NAMES: &[&str] = &[
    "Host",
    "Connection",
    "Content-Length",
    "Pragma",
    "Cache-Control",
    "sec-ch-ua",
    "X-Debug-Options",
    "sec-ch-ua-mobile",
    "Authorization",
    "User-Agent",
    "Content-Type",
    "X-Captcha-Key",
    "X-Captcha-Session-Id",
    "X-Captcha-Rqtoken",
    "X-Context-Properties",
    "X-Discord-Locale",
    "X-Discord-Timezone",
    "X-Fingerprint",
    "X-Installation-Id",
    "X-Super-Properties",
    "sec-ch-ua-platform",
    "Accept",
    "Origin",
    "Sec-Fetch-Site",
    "Sec-Fetch-Mode",
    "Sec-Fetch-Dest",
    "Sec-Fetch-User",
    "Upgrade-Insecure-Requests",
    "Referer",
    "Accept-Encoding",
    "Accept-Language",
    "Priority",
    "Cookie",
];

/// El `OrigHeaderMap` para pasarle a `RequestBuilder::orig_headers`.
pub fn chrome_header_case() -> OrigHeaderMap {
    let mut map = OrigHeaderMap::new();
    for name in HEADER_NAMES {
        map.insert(*name);
    }
    map
}
