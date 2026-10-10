//! Cliente WebSocket común a todo `ecord` (Gateway, login por QR y voz).
//!
//! Antes cada uno abría su socket con `tokio-tungstenite` (TLS de rustls,
//! huella propia). Ahora todos pasan por el mismo `wreq::Client` con la
//! emulación de Chrome que usa el REST vendorizado, así que el tráfico de la
//! cuenta se ve como un solo navegador y hay una única pila TLS para sockets.
//!
//! Con `ECORD_INSECURE_TLS=1` también se omite la verificación del
//! certificado en estos sockets (ver `client`), igual que en el REST.
//!
//! # El handshake NO lleva headers propios
//!
//! Es a propósito. Contra `gateway.discord.gg` se probó, y el resultado fue:
//!
//! - Emulación sola (sus headers, nada propio): el upgrade pasa. Es también lo
//!   que hace la crate de referencia `discord_client_gateway`.
//! - Emulación + `Origin`: `403`.
//! - Emulación sin sus headers + los de un navegador real (`Origin`,
//!   `User-Agent` de Chrome/150, `Accept-Language`, `Pragma`, `Cache-Control`,
//!   `Accept-Encoding`): `403`.
//! - Lo mismo con el `User-Agent` de la emulación (Chrome/149): `403`.
//!
//! Por eso `connect` no acepta headers: el Gateway no necesita `Origin` y
//! cualquier combinación propia se rechazó. No volver a agregarlos sin
//! probarlo contra el Gateway real. Consecuencia: el `User-Agent` y el
//! `Accept-Language` del handshake son los de la emulación (Chrome/149,
//! `en-US`), no los del fingerprint, que siguen yendo en el REST y el
//! `IDENTIFY`.
//!
//! Sospecha sin comprobar de por qué falla lo propio: el Gateway (o
//! Cloudflare) compara también la forma del request en el cable (mayúsculas y
//! orden de los headers) con la que tendría Chrome, y la emulación la
//! reproduce solo para sus propios headers. Si hiciera falta mandar algo
//! propio, habría que darle a `wreq` esa información (`OrigHeaderMap`).
//!
//! `Message` y `WebSocket` se reexportan desde acá para que el resto del
//! código no dependa de la ruta interna de `wreq`.

use std::sync::OnceLock;

use wreq_util::{Emulation, Platform, Profile};

pub(crate) use wreq::ws::WebSocket;
pub(crate) use wreq::ws::message::Message;

/// Cliente `wreq` compartido (se arma una sola vez y se reusa entre
/// reconexiones y entre todos los sockets).
///
/// La emulación tiene que coincidir con la del REST vendorizado
/// (`discord_client_rest::bootstrap::build_emulated_client`: Chrome149 en
/// Windows); si cambia allá, cambiarla acá también.
fn client() -> Result<&'static wreq::Client, wreq::Error> {
    static CLIENT: OnceLock<wreq::Client> = OnceLock::new();
    if let Some(client) = CLIENT.get() {
        return Ok(client);
    }
    let emulation = Emulation::builder().profile(Profile::Chrome149).platform(Platform::Windows).build();
    let mut builder = wreq::Client::builder()
        .emulation(emulation)
        .gzip(true)
        .deflate(true)
        .brotli(true)
        .zstd(true);

    // Debug: igual que el REST (`bootstrap::build_emulated_client`), con
    // `ECORD_INSECURE_TLS=1` se apaga la verificación de certificados para
    // poder interceptar también los WebSockets (Gateway, QR y voz) con
    // HTTP Toolkit / mitmproxy. Usa la MISMA comprobación que el REST y que
    // el aviso de seguridad de la UI, así que los tres siempre coinciden.
    // El cliente se crea una sola vez: cambiar la variable exige reiniciar.
    if discord_client_rest::insecure_tls_enabled() {
        log::warn!(
            "ECORD_INSECURE_TLS=1: verificación de certificados TLS desactivada en los WebSockets, solo para debug"
        );
        builder = builder.tls_cert_verification(false);
    }

    let client = builder.build()?;
    Ok(CLIENT.get_or_init(|| client))
}

/// Abre un WebSocket (`ws://` o `wss://`). Los headers del handshake son los
/// de la emulación (ver la nota del módulo).
pub(crate) async fn connect(url: &str) -> Result<WebSocket, wreq::Error> {
    let result = match client()?.websocket(url).send().await {
        Ok(response) => response.into_websocket().await,
        Err(error) => Err(error),
    };
    if let Err(error) = &result {
        // El detalle (`Debug`) trae más que el mensaje corto: por ejemplo el
        // `403` de un upgrade rechazado por Cloudflare.
        log::warn!("handshake WebSocket a {url} falló: {error:?}");
    }
    result
}
