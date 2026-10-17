//! Cliente WebSocket común a todo `ecord` (Gateway, login por QR y voz).
//!
//! Todos los sockets pasan por el mismo `wreq::Client` con la
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

use std::pin::Pin;
use std::sync::OnceLock;
use std::task::{Context, Poll};

use futures_util::sink::Sink;
use futures_util::stream::Stream;
use wreq_util::{Emulation, Platform, Profile};

use wreq::header::OrigHeaderMap;
use wreq::ws::WebSocket as RawWebSocket;
pub(crate) use wreq::ws::message::Message;

use discord_client_rest::netlog::{self, Dir, FrameType};
/// Tipo de conexión que se declara al abrir un socket (ver `connect`); es lo
/// que la pestaña Tráfico de las devtools muestra como "tipo".
pub(crate) use discord_client_rest::netlog::Kind as NetKind;

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
    let emulation = Emulation::builder()
        .profile(Profile::Chrome149)
        .platform(Platform::Windows)
        .build();
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

/// Grafía y orden de los headers del handshake WebSocket tal como los manda
/// Chrome por HTTP/1.1 (el upgrade a WebSocket NO va por HTTP/2, así que acá
/// sí se ve la grafía: `Title-Case`, con `Sec-WebSocket-*` en mayúsculas
/// internas). Sin esto wreq los manda todos en minúscula, que es lo que un
/// navegador real nunca hace en HTTP/1.1 y lo que delata al cliente.
///
/// `OrigHeaderMap` solo fija CÓMO se escribe cada nombre y en qué orden; los
/// valores siguen siendo los de la emulación o los que se agreguen aparte.
/// El orden es el de Chrome en el cable (las capturas de HTTP Toolkit lo
/// muestran ordenado alfabéticamente, no es el real).
pub(crate) fn browser_ws_header_case() -> OrigHeaderMap {
    let mut map = OrigHeaderMap::new();
    for name in [
        "Host",
        "Connection",
        "Pragma",
        "Cache-Control",
        "User-Agent",
        "Upgrade",
        "Origin",
        "Sec-WebSocket-Version",
        "Accept-Encoding",
        "Accept-Language",
        "Sec-WebSocket-Key",
        "Sec-WebSocket-Extensions",
    ] {
        map.insert(name);
    }
    map
}

/// WebSocket de `wreq` con el registro de tráfico de las devtools
/// (`discord_client_rest::netlog`) puesto en el medio.
///
/// Implementa `Stream` y `Sink<Message>` igual que el socket de `wreq` y se
/// puede usar igual (`split()`, `next()`, `send()`), así que Gateway, voz,
/// streams y login por QR no cambian: cada mensaje que sale o llega queda
/// registrado con su sentido. Con las devtools apagadas `conn` vale 0 y el
/// costo es una comparación por mensaje.
pub(crate) struct WebSocket {
    // `Pin<Box<_>>` hace al conjunto `Unpin` sin importar si el socket de
    // `wreq` lo es, y deja delegar con `as_mut()`.
    inner: Pin<Box<RawWebSocket>>,
    conn: u64,
}

impl WebSocket {
    fn new(inner: RawWebSocket, kind: NetKind, url: &str, who: &str) -> Self {
        Self {
            inner: Box::pin(inner),
            conn: netlog::conn_open(kind, url, who),
        }
    }

    /// Id de esta conexión en el registro de tráfico (0 si está apagado).
    pub(crate) fn conn_id(&self) -> u64 {
        self.conn
    }
}

impl Drop for WebSocket {
    fn drop(&mut self) {
        // No hace nada si ya se registró el cierre.
        netlog::conn_close(self.conn, "conexión descartada por el cliente");
    }
}

/// Registra un mensaje de WebSocket.
fn trace(conn: u64, dir: Dir, message: &Message) {
    if conn == 0 {
        return;
    }
    match message {
        Message::Text(text) => netlog::frame_text(conn, dir, text),
        Message::Binary(bytes) => netlog::frame_binary(conn, dir, bytes),
        Message::Ping(bytes) => netlog::frame_control(conn, dir, FrameType::Ping, bytes.len(), ""),
        Message::Pong(bytes) => netlog::frame_control(conn, dir, FrameType::Pong, bytes.len(), ""),
        Message::Close(frame) => {
            let detail = match frame {
                Some(frame) => format!("código {} {}", u16::from(frame.code.clone()), frame.reason),
                None => "sin código".to_owned(),
            };
            netlog::frame_control(conn, dir, FrameType::Close, 0, &detail);
        }
        _ => {}
    }
}

/// Registra lo que devolvió una lectura del socket.
fn trace_read<E: std::fmt::Display>(conn: u64, item: &Option<Result<Message, E>>) {
    if conn == 0 {
        return;
    }
    match item {
        Some(Ok(message)) => {
            trace(conn, Dir::In, message);
            if let Message::Close(_) = message {
                netlog::conn_close(conn, "cierre recibido del servidor");
            }
        }
        Some(Err(error)) => netlog::conn_note(conn, &format!("Error de lectura: {error}")),
        None => netlog::conn_close(conn, "el servidor terminó el flujo"),
    }
}

impl Stream for WebSocket {
    type Item = <RawWebSocket as Stream>::Item;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this = self.get_mut();
        let poll = Stream::poll_next(this.inner.as_mut(), cx);
        if let Poll::Ready(item) = &poll {
            trace_read(this.conn, item);
        }
        poll
    }
}

impl Sink<Message> for WebSocket {
    type Error = <RawWebSocket as Sink<Message>>::Error;

    fn poll_ready(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        <RawWebSocket as Sink<Message>>::poll_ready(self.get_mut().inner.as_mut(), cx)
    }

    fn start_send(self: Pin<&mut Self>, item: Message) -> Result<(), Self::Error> {
        let this = self.get_mut();
        trace(this.conn, Dir::Out, &item);
        <RawWebSocket as Sink<Message>>::start_send(this.inner.as_mut(), item)
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        <RawWebSocket as Sink<Message>>::poll_flush(self.get_mut().inner.as_mut(), cx)
    }

    fn poll_close(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        let this = self.get_mut();
        let poll = <RawWebSocket as Sink<Message>>::poll_close(this.inner.as_mut(), cx);
        if let Poll::Ready(Ok(())) = &poll {
            netlog::conn_close(this.conn, "cerrada por el cliente");
        }
        poll
    }
}

/// Abre un WebSocket (`ws://` o `wss://`). Los headers del handshake son los
/// de la emulación (ver la nota del módulo).
///
/// `kind` y `who` son para el registro de tráfico de las devtools: qué clase
/// de conexión es y qué parte de la app la abre.
pub(crate) async fn connect(url: &str, kind: NetKind, who: &str) -> Result<WebSocket, wreq::Error> {
    let result = match client()?.websocket(url).orig_headers(browser_ws_header_case()).send().await {
        Ok(response) => response.into_websocket().await,
        Err(error) => Err(error),
    };
    match result {
        Ok(socket) => Ok(WebSocket::new(socket, kind, url, who)),
        Err(error) => {
            // El detalle (`Debug`) trae más que el mensaje corto: por ejemplo el
            // `403` de un upgrade rechazado por Cloudflare.
            log::warn!("handshake WebSocket a {url} falló: {error:?}");
            netlog::conn_failed(kind, url, who, &format!("{error}"));
            Err(error)
        }
    }
}

/// Abre el WebSocket del login por QR con `wreq` (misma emulación de Chrome
/// que el resto) y `headers` propios en el handshake, escritos con la grafía
/// y el orden de un navegador real (`browser_ws_header_case`).
///
/// Antes esto salía por `tokio-tungstenite` porque con `wreq` el servidor
/// devolvía `403`: los headers propios se mandaban todos en minúscula, algo
/// que Chrome no hace en HTTP/1.1. Con la grafía corregida el handshake pasa
/// y ya no hace falta una segunda pila de WebSocket.
pub(crate) async fn connect_qr(url: &str, headers: &[(&'static str, String)]) -> anyhow::Result<WebSocket> {
    let who = "remote_auth (login por QR)";
    let mut request = client()?.websocket(url).orig_headers(browser_ws_header_case());
    for (name, value) in headers {
        request = request.header(*name, value.as_str());
    }
    let result = match request.send().await {
        Ok(response) => response.into_websocket().await,
        Err(error) => Err(error),
    };
    match result {
        Ok(socket) => Ok(WebSocket::new(socket, NetKind::RemoteAuth, url, who)),
        Err(error) => {
            log::warn!("handshake WebSocket del QR a {url} falló: {error:?}");
            netlog::conn_failed(NetKind::RemoteAuth, url, who, &format!("{error}"));
            Err(anyhow::anyhow!("handshake del QR rechazado: {error}"))
        }
    }
}
