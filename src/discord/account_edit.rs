//! Cambios de la cuenta que se mandan a Discord desde Ajustes: nombre visible,
//! usuario, correo, contraseña, avatar, "Sobre mí", pronombres, banner y los
//! dispositivos con sesión abierta.
//!
//! Cada pedido sale en un hilo aparte (igual que `spawn_patch_user_settings`) y
//! su resultado vuelve a la UI como un `AppEvent`; `lib::state::account_edit`
//! lo aplica a `App`. Los interruptores y "quién puede enviarte solicitudes" NO
//! pasan por acá: van por `settings-proto` (`spawn_patch_user_settings`).

use std::sync::mpsc::Sender;

use serde_json::Value;

use super::{uwu_rest, AppEvent};

/// A qué endpoint va un cambio de cuenta.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AccountEditTarget {
    /// `PATCH /users/@me`
    Me,
    /// `PATCH /users/@me/profile`
    Profile,
}

/// Un cambio listo para mandar.
#[derive(Clone, Debug)]
pub struct AccountEdit {
    pub target: AccountEditTarget,
    pub body: Value,
    /// Lo que se le avisa a la persona cuando Discord lo acepta.
    pub done: &'static str,
}

/// Lo que devolvió Discord al aceptar un cambio.
#[derive(Debug)]
pub struct AccountEditOutcome {
    pub target: AccountEditTarget,
    /// Lo que se mandó (sin contraseñas ni imágenes): sirve de respaldo para
    /// actualizar lo que Discord no repite en su respuesta.
    pub sent: Value,
    /// El cuerpo de la respuesta, tal cual.
    pub response: Value,
    /// Token nuevo (solo al cambiar la contraseña).
    pub new_token: Option<String>,
}

/// Un dispositivo con sesión abierta (`GET /auth/sessions`).
#[derive(Clone, Debug, Default)]
pub struct AuthSession {
    pub id_hash: String,
    pub os: String,
    pub platform: String,
    pub location: String,
    /// RFC 3339.
    pub last_used: String,
}

/// `{ "user_sessions": [ { id_hash, approx_last_used_time, client_info: { os,
/// platform, location } } ] }` → lista de [`AuthSession`], la más reciente
/// primero.
pub fn parse_sessions(value: &Value) -> Vec<AuthSession> {
    let text = |v: &Value, key: &str| v.get(key).and_then(Value::as_str).unwrap_or_default().to_owned();
    let mut list: Vec<AuthSession> = value
        .get("user_sessions")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(|item| {
                    let id_hash = text(item, "id_hash");
                    if id_hash.is_empty() {
                        return None;
                    }
                    let info = item.get("client_info").cloned().unwrap_or(Value::Null);
                    Some(AuthSession {
                        id_hash,
                        os: text(&info, "os"),
                        platform: text(&info, "platform"),
                        location: text(&info, "location"),
                        last_used: text(item, "approx_last_used_time"),
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    // Las fechas RFC 3339 en UTC se ordenan bien como texto.
    list.sort_by(|a, b| b.last_used.cmp(&a.last_used));
    list
}

/// Lo que se mandó, sin lo que no debe quedarse en memoria ni sirve para
/// actualizar la vista (contraseñas e imágenes en base64).
fn sendable_summary(body: &Value) -> Value {
    let mut copy = body.clone();
    if let Some(map) = copy.as_object_mut() {
        for key in ["password", "new_password", "avatar", "banner"] {
            map.remove(key);
        }
    }
    copy
}

/// Manda los cambios EN ORDEN (si uno falla, los siguientes igual salen: cada
/// uno avisa su propio resultado) con el token de la cuenta.
pub fn spawn_account_edits(token: String, edits: Vec<AccountEdit>, tx: Sender<AppEvent>) {
    std::thread::spawn(move || {
        let _who = discord_client_rest::netlog::origin("account_edit");
        let Ok(rt) = tokio::runtime::Builder::new_current_thread().enable_all().build() else {
            for edit in edits {
                let _ = tx.send(AppEvent::AccountEditResult {
                    done: edit.done,
                    result: Err("No se pudo iniciar la conexión con Discord.".to_owned()),
                });
            }
            return;
        };
        rt.block_on(async move {
            let rest = match uwu_rest::UwuRest::for_token(token).await {
                Ok(rest) => rest,
                Err(e) => {
                    for edit in edits {
                        let _ = tx.send(AppEvent::AccountEditResult {
                            done: edit.done,
                            result: Err(e.to_string()),
                        });
                    }
                    return;
                }
            };
            for edit in edits {
                let sent = sendable_summary(&edit.body);
                let response = match edit.target {
                    AccountEditTarget::Me => rest.patch_me(edit.body.clone()).await,
                    AccountEditTarget::Profile => rest.patch_my_profile(edit.body.clone()).await,
                };
                let result = response.map_err(|e| e.to_string()).map(|response| {
                    let new_token = response
                        .get("token")
                        .and_then(Value::as_str)
                        .filter(|t| !t.is_empty())
                        .map(str::to_owned);
                    AccountEditOutcome {
                        target: edit.target,
                        sent,
                        response,
                        new_token,
                    }
                });
                let _ = tx.send(AppEvent::AccountEditResult {
                    done: edit.done,
                    result,
                });
            }
        });
    });
}

/// Pide los dispositivos con sesión abierta.
pub fn spawn_fetch_sessions(token: String, tx: Sender<AppEvent>) {
    std::thread::spawn(move || {
        let _who = discord_client_rest::netlog::origin("fetch_auth_sessions");
        let Ok(rt) = tokio::runtime::Builder::new_current_thread().enable_all().build() else {
            let _ = tx.send(AppEvent::AuthSessions(Err("No se pudo iniciar la conexión.".to_owned())));
            return;
        };
        rt.block_on(async move {
            let result = match uwu_rest::UwuRest::for_token(token).await {
                Ok(rest) => rest
                    .get_auth_sessions()
                    .await
                    .map(|value| parse_sessions(&value))
                    .map_err(|e| e.to_string()),
                Err(e) => Err(e.to_string()),
            };
            let _ = tx.send(AppEvent::AuthSessions(result));
        });
    });
}

/// Cierra la sesión de un dispositivo.
pub fn spawn_logout_session(token: String, id_hash: String, tx: Sender<AppEvent>) {
    std::thread::spawn(move || {
        let _who = discord_client_rest::netlog::origin("logout_auth_session");
        let Ok(rt) = tokio::runtime::Builder::new_current_thread().enable_all().build() else {
            let _ = tx.send(AppEvent::AuthSessionClosed {
                id_hash,
                result: Err("No se pudo iniciar la conexión.".to_owned()),
            });
            return;
        };
        rt.block_on(async move {
            let result = match uwu_rest::UwuRest::for_token(token).await {
                Ok(rest) => rest
                    .logout_auth_sessions(std::slice::from_ref(&id_hash))
                    .await
                    .map_err(|e| e.to_string()),
                Err(e) => Err(e.to_string()),
            };
            let _ = tx.send(AppEvent::AuthSessionClosed { id_hash, result });
        });
    });
}
