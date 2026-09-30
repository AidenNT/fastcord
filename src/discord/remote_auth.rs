//! Login por código QR ("remote auth"), el mismo mecanismo que usa el
//! cliente oficial de escritorio: mostrás un QR, lo escaneás con Discord en
//! el celular (Ajustes de usuario → "Escanear código QR" / lo que aparece
//! al tocar el ícono de QR en la pantalla de login del celular) y el
//! celular le pasa un token de sesión al escritorio sin que nunca tengas
//! que tipear usuario/contraseña acá.
//!
//! ⚠️ Este protocolo NO es público ni está documentado por Discord: es
//! reverse-engineered (lo mismo que usan clientes no oficiales como
//! discord.js-selfbot-v13 / discord.py-self). Estos son los pasos tal como
//! están documentados por esos proyectos a la fecha de este código, pero
//! Discord puede cambiarlo sin aviso — si algo no matchea (un campo con
//! otro nombre, otro `op`), vas a necesitar capturar el tráfico real del
//! cliente oficial (Wireshark / proxy) y ajustar esta función. No pude
//! probar esto contra los servidores reales de Discord (mi sandbox no
//! tiene acceso a internet), así que tratalo como un primer borrador a
//! depurar, no como código garantizado.
//!
//! Pasos:
//! 1. Conectar a `wss://remote-auth-gateway.discord.gg/?v=2`.
//! 2. Recibir `hello` → generar un par de claves RSA-2048 y mandar `init`
//!    con la clave pública (SPKI, en base64).
//! 3. Recibir `nonce_proof` con un nonce cifrado con esa clave pública →
//!    descifrarlo, hashearlo (SHA-256) y devolver el hash como prueba.
//! 4. Recibir `pending_remote_init` con un `fingerprint` → verificamos que
//!    sea de verdad el hash SHA-256 de nuestra propia clave pública (si no
//!    matchea, alguien en el medio está sustituyendo la clave — cortar en
//!    vez de seguir) y armamos la URL `https://discord.com/ra/{fingerprint}`,
//!    que es lo que codificamos como QR.
//! 5. Cuando escaneás con el celular: `pending_ticket` trae los datos del
//!    usuario cifrados (para mostrar "¿sos vos?") y luego, tras confirmar
//!    en el celular, `pending_login` trae un `ticket`.
//! 6. Ese `ticket` se canjea por `POST /api/v10/users/@me/remote-auth/login`
//!    → devuelve `encrypted_token`, que se descifra con la misma clave
//!    privada para obtener el token real de la cuenta.
//!
//! Lo que se agregó sobre la primera versión de este archivo (mirando
//! cómo lo resuelve Concord, `discord/qr_auth.rs` ahí): mandar los
//! headers/fingerprint reales en la conexión y en el canje del ticket (en
//! vez de un `reqwest::Client` nuevo y sin headers), verificar el
//! fingerprint del server antes de confiar en él, y un puñado de
//! reintentos si la conexión se corta antes de terminar — a diferencia
//! del Gateway principal esto es un flujo corto (dura lo que tarda
//! escanear un QR), así que no hace falta la reconexión indefinida de
//! `gateway::run`, con 3 intentos alcanza.

use std::sync::Arc;

use base64::{Engine as _, engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD}};
use futures_util::{SinkExt, StreamExt};
use rsa::pkcs8::EncodePublicKey;
use rsa::{Oaep, RsaPrivateKey, RsaPublicKey};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::Message;

use crate::discord::auth_http::{discord_login_headers, DiscordAuthSession};
use crate::discord::fingerprint::{self, ClientFingerprint};
use crate::discord::AppEvent;

const REMOTE_AUTH_URL: &str = "wss://remote-auth-gateway.discord.gg/?v=2";
const TICKET_EXCHANGE_URL: &str = "https://discord.com/api/v10/users/@me/remote-auth/login";
const MAX_ATTEMPTS: u8 = 3;

#[derive(Deserialize)]
struct Frame {
    op: String,
    #[serde(default)]
    heartbeat_interval: Option<u64>,
    #[serde(default)]
    encrypted_nonce: Option<String>,
    #[serde(default)]
    fingerprint: Option<String>,
    #[serde(default)]
    encrypted_user_payload: Option<String>,
    #[serde(default)]
    ticket: Option<String>,
}

pub async fn run(fingerprint: Arc<ClientFingerprint>, tx: std::sync::mpsc::Sender<AppEvent>) -> anyhow::Result<String> {
    let auth_session = DiscordAuthSession::new(fingerprint);
    let mut last_error = None;
    for attempt in 1..=MAX_ATTEMPTS {
        match run_once(&auth_session, &tx).await {
            Ok(token) => return Ok(token),
            Err(e) if attempt < MAX_ATTEMPTS => {
                log::warn!("Remote auth intento {attempt}/{MAX_ATTEMPTS} falló: {e}");
                last_error = Some(e);
                tokio::time::sleep(std::time::Duration::from_secs(1)).await;
            }
            Err(e) => last_error = Some(e),
        }
    }
    Err(last_error.unwrap_or_else(|| anyhow::anyhow!("remote auth falló sin un error específico")))
}

async fn run_once(auth_session: &DiscordAuthSession, tx: &std::sync::mpsc::Sender<AppEvent>) -> anyhow::Result<String> {
    let mut rng = rand::thread_rng();
    let priv_key = RsaPrivateKey::new(&mut rng, 2048)?;
    let pub_key = RsaPublicKey::from(&priv_key);
    let pub_key_der = pub_key.to_public_key_der()?;
    let encoded_public_key = STANDARD.encode(pub_key_der.as_bytes());
    // Lo que Discord tendría que devolvernos en `pending_remote_init` si
    // nadie en el medio sustituyó la clave pública que mandamos en
    // `init` — ver el comentario de más arriba.
    let expected_fingerprint = URL_SAFE_NO_PAD.encode(Sha256::digest(pub_key_der.as_bytes()));

    let mut request = REMOTE_AUTH_URL.into_client_request()?;
    request.headers_mut().extend(fingerprint::discord_gateway_headers(auth_session.fingerprint()));
    let (ws_stream, _) = tokio_tungstenite::connect_async(request).await?;
    let (mut write, mut read) = ws_stream.split();

    // El QR tarda un rato en escanearse, así que hay que mandar heartbeats
    // mientras se espera o el Gateway corta la conexión. Arranca "nunca" y
    // se ajusta al valor real apenas llega `hello`.
    let mut heartbeat = tokio::time::interval(std::time::Duration::from_secs(3600));
    heartbeat.tick().await; // el primer tick es inmediato; lo consumimos ya
    let mut ticket_for_login: Option<String> = None;

    loop {
        let msg = tokio::select! {
            msg = read.next() => match msg {
                Some(msg) => msg?,
                None => break,
            },
            _ = heartbeat.tick() => {
                let hb = serde_json::json!({ "op": "heartbeat" });
                write.send(Message::Text(hb.to_string().into())).await?;
                continue;
            }
        };
        let Message::Text(text) = msg else { continue };
        let frame: Frame = serde_json::from_str(&text)?;

        match frame.op.as_str() {
            "hello" => {
                if let Some(interval_ms) = frame.heartbeat_interval {
                    heartbeat = tokio::time::interval(std::time::Duration::from_millis(interval_ms));
                    heartbeat.tick().await;
                }
                let init = serde_json::json!({
                    "op": "init",
                    "encoded_public_key": encoded_public_key,
                });
                write.send(Message::Text(init.to_string().into())).await?;
            }
            "nonce_proof" => {
                let Some(encrypted_nonce) = &frame.encrypted_nonce else { continue };
                let nonce_bytes = STANDARD.decode(encrypted_nonce)?;
                let decrypted = priv_key.decrypt(Oaep::new::<Sha256>(), &nonce_bytes)?;
                let hash = Sha256::digest(&decrypted);
                let proof = URL_SAFE_NO_PAD.encode(hash);
                let reply = serde_json::json!({ "op": "nonce_proof", "proof": proof });
                write.send(Message::Text(reply.to_string().into())).await?;
            }
            "pending_remote_init" => {
                let Some(server_fingerprint) = &frame.fingerprint else { continue };
                if server_fingerprint != &expected_fingerprint {
                    // No es un error de red: alguien está mandando un
                    // fingerprint que no corresponde a la clave pública
                    // que mandamos nosotros. Cortar acá, no seguir el
                    // handshake con una clave que no controlamos.
                    anyhow::bail!(
                        "el fingerprint que mandó el Gateway de remote auth no coincide con el esperado (posible manipulación de la conexión)"
                    );
                }
                let url = format!("https://discord.com/ra/{server_fingerprint}");
                let _ = tx.send(AppEvent::QrCode(url));
            }
            "pending_ticket" => {
                // El usuario ya escaneó el QR con el celular. Descifra el
                // payload solo para poder mostrar "¿confirmás que sos
                // <usuario>?" en pantalla; el login recién se completa
                // cuando confirme ahí.
                if let Some(encrypted) = &frame.encrypted_user_payload {
                    if let Ok(bytes) = STANDARD.decode(encrypted) {
                        if let Ok(payload) = priv_key.decrypt(Oaep::new::<Sha256>(), &bytes) {
                            if let Ok(text) = String::from_utf8(payload) {
                                // Formato observado: "id:discriminator:avatar:username"
                                let username = text
                                    .split(':')
                                    .last()
                                    .unwrap_or("tu cuenta de Discord")
                                    .to_string();
                                let _ = tx.send(AppEvent::QrConfirming { username });
                            }
                        }
                    }
                }
                ticket_for_login = frame.ticket.clone();
            }
            "pending_login" => {
                ticket_for_login = frame.ticket.clone().or(ticket_for_login);
                break;
            }
            "cancel" => {
                anyhow::bail!("El login por QR fue cancelado (¿expiró el código o lo cancelaste en el celular?)");
            }
            _ => {}
        }
    }

    let ticket = ticket_for_login
        .ok_or_else(|| anyhow::anyhow!("El Gateway cerró la conexión antes de mandar el ticket de login"))?;

    // Canjear el ticket por el token real vía REST — con el mismo
    // cliente (y por lo tanto los mismos headers/cookies) que el resto
    // del flujo de auth, no un `reqwest::Client` nuevo y anónimo.
    #[derive(Deserialize)]
    struct LoginResponse {
        encrypted_token: String,
    }
    let resp: LoginResponse = auth_session
        .http()
        .post(TICKET_EXCHANGE_URL)
        .headers(discord_login_headers(auth_session.fingerprint()))
        .json(&serde_json::json!({ "ticket": ticket }))
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;

    let token_bytes = STANDARD.decode(resp.encrypted_token)?;
    let token = priv_key.decrypt(Oaep::new::<Sha256>(), &token_bytes)?;
    Ok(String::from_utf8(token)?)
}
