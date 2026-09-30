//! Login con usuario/contraseña (y el 2FA que pida Discord después),
//! como alternativa al QR de `remote_auth`. Portado de
//! `discord/password_auth.rs` de Concord — esa versión ya está probada
//! (tiene sus propios tests, que se trajeron también) y encima ya
//! encajaba con lo que este proyecto tenía armado pero sin conectar:
//! `DiscordAuthSession`/`discord_login_headers` (`discord/auth_http.rs`)
//! existían acá de antes, letra por letra iguales a como los usa este
//! archivo en Concord.
//!
//! A diferencia del original: acá no hay un canal de eventos aparte
//! (`tokio::sync::mpsc::Sender<PasswordAuthEvent>`) — todo se manda por el
//! mismo `std::sync::mpsc::Sender<AppEvent>` de siempre, como hace
//! `remote_auth`. Las funciones `login_with_password`/`verify_mfa`/
//! `send_mfa_sms`/`parse_login_success`/`format_login_error` internas se
//! trajeron básicamente tal cual (son las que hacen el trabajo real
//! contra la API de Discord y no dependen del mecanismo de eventos).
//!
//! ⚠️ Mismo aviso que en `remote_auth.rs`: loguearse con el token de una
//! cuenta de usuario (no un bot) es un "self-bot" para Discord, va contra
//! sus términos de servicio, y además esto no lo pude probar contra
//! servidores reales (sandbox sin red) — tratalo como primer borrador.

use serde::Deserialize;
use serde_json::{Value, json};

use crate::discord::AppEvent;
use crate::discord::auth_http::{DiscordAuthSession, discord_login_headers};

const LOGIN_URL: &str = "https://discord.com/api/v10/auth/login";
const SUSPENDED_ACCOUNT_ERROR: &str =
    "Discord dice que esta cuenta está suspendida. Hay que resolverlo desde el Safety Hub de Discord.";
const MFA_VERIFY_URL: &str = "https://discord.com/api/v10/auth/mfa";
const MFA_SMS_SEND_URL: &str = "https://discord.com/api/v10/auth/mfa/sms/send";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MfaMethod {
    Totp,
    Sms,
}

impl MfaMethod {
    fn endpoint_name(self) -> &'static str {
        match self {
            Self::Totp => "totp",
            Self::Sms => "sms",
        }
    }
}

/// Lo que hace falta guardar mientras se espera que el usuario escriba
/// (o pida por SMS y después escriba) el código de 2FA — ver
/// `AuthStatus::PasswordMfaRequired`/`App::pending_mfa` en `lib/state.rs`.
#[derive(Clone, Debug)]
pub struct MfaChallenge {
    pub ticket: String,
    pub login_instance_id: String,
    pub methods: Vec<MfaMethod>,
}

/// Arranca el login con usuario/contraseña en un hilo de fondo (mismo
/// patrón que `discord::spawn_login_flow`: hilo de SO propio + runtime de
/// tokio propio). El resultado llega por `tx` como uno de los
/// `AppEvent::Password*`.
pub fn spawn(login: String, password: String, tx: std::sync::mpsc::Sender<AppEvent>) {
    std::thread::spawn(move || {
        let Ok(rt) = tokio::runtime::Builder::new_current_thread().enable_all().build() else {
            let _ = tx.send(AppEvent::Error("No se pudo iniciar el runtime async".into()));
            return;
        };
        rt.block_on(async move {
            let fingerprint = crate::discord::init_fingerprint().await;
            let auth_session = DiscordAuthSession::new(fingerprint);
            match login_with_password(&login, &password, &auth_session).await {
                Ok(LoginOutcome::Token(token)) => {
                    let _ = tx.send(AppEvent::PasswordLoginToken(token));
                }
                Ok(LoginOutcome::MfaRequired(challenge)) => {
                    let _ = tx.send(AppEvent::PasswordMfaRequired(challenge));
                }
                Ok(LoginOutcome::RequiredActions(actions)) => {
                    let _ = tx.send(AppEvent::PasswordLoginBlocked(actions));
                }
                Err(error) => {
                    let _ = tx.send(AppEvent::PasswordLoginFailed(error));
                }
            }
        });
    });
}

/// Manda el código de 2FA (TOTP, o el que llegó por SMS) para terminar el
/// login empezado con `spawn`.
pub fn spawn_mfa_verify(
    method: MfaMethod,
    code: String,
    ticket: String,
    login_instance_id: String,
    tx: std::sync::mpsc::Sender<AppEvent>,
) {
    std::thread::spawn(move || {
        let Ok(rt) = tokio::runtime::Builder::new_current_thread().enable_all().build() else {
            let _ = tx.send(AppEvent::Error("No se pudo iniciar el runtime async".into()));
            return;
        };
        rt.block_on(async move {
            let fingerprint = crate::discord::init_fingerprint().await;
            let auth_session = DiscordAuthSession::new(fingerprint);
            match verify_mfa(method, &code, &ticket, &login_instance_id, &auth_session).await {
                Ok(token) => {
                    let _ = tx.send(AppEvent::PasswordLoginToken(token));
                }
                Err(error) => {
                    let _ = tx.send(AppEvent::PasswordLoginFailed(error));
                }
            }
        });
    });
}

/// Pide que Discord mande el código de 2FA por SMS (cuando `Sms` es uno
/// de los métodos disponibles del desafío).
pub fn spawn_mfa_sms(ticket: String, tx: std::sync::mpsc::Sender<AppEvent>) {
    std::thread::spawn(move || {
        let Ok(rt) = tokio::runtime::Builder::new_current_thread().enable_all().build() else {
            let _ = tx.send(AppEvent::Error("No se pudo iniciar el runtime async".into()));
            return;
        };
        rt.block_on(async move {
            let fingerprint = crate::discord::init_fingerprint().await;
            let auth_session = DiscordAuthSession::new(fingerprint);
            match send_mfa_sms(&ticket, &auth_session).await {
                Ok(phone) => {
                    let _ = tx.send(AppEvent::PasswordMfaSmsSent(phone));
                }
                Err(error) => {
                    let _ = tx.send(AppEvent::PasswordLoginFailed(error));
                }
            }
        });
    });
}

enum LoginOutcome {
    Token(String),
    MfaRequired(MfaChallenge),
    RequiredActions(Vec<String>),
}

async fn login_with_password(
    login: &str,
    password: &str,
    auth_session: &DiscordAuthSession,
) -> Result<LoginOutcome, String> {
    let response = auth_session
        .http()
        .post(LOGIN_URL)
        .headers(discord_login_headers(auth_session.fingerprint()))
        .json(&json!({
            "login": normalize_login_identifier(login),
            "password": password,
        }))
        .send()
        .await
        .map_err(|error| format!("No se pudo mandar el login a Discord: {error}"))?;

    let status = response.status();
    let body = response
        .text()
        .await
        .map_err(|error| format!("No se pudo leer la respuesta de Discord: {error}"))?;

    if status.is_success() {
        parse_login_success(&body)
    } else {
        Err(format_login_error(status, &body))
    }
}

async fn send_mfa_sms(ticket: &str, auth_session: &DiscordAuthSession) -> Result<Option<String>, String> {
    #[derive(Deserialize)]
    struct SmsResponse {
        phone: Option<String>,
    }

    let response = auth_session
        .http()
        .post(MFA_SMS_SEND_URL)
        .headers(discord_login_headers(auth_session.fingerprint()))
        .json(&json!({ "ticket": ticket }))
        .send()
        .await
        .map_err(|error| format!("No se pudo pedir el SMS a Discord: {error}"))?;

    let status = response.status();
    let body = response
        .text()
        .await
        .map_err(|error| format!("No se pudo leer la respuesta de Discord: {error}"))?;

    if status.is_success() {
        let response: SmsResponse = serde_json::from_str(&body)
            .map_err(|error| format!("No se pudo interpretar la respuesta del SMS: {error}"))?;
        Ok(response.phone)
    } else {
        Err(format_login_error(status, &body))
    }
}

async fn verify_mfa(
    method: MfaMethod,
    code: &str,
    ticket: &str,
    login_instance_id: &str,
    auth_session: &DiscordAuthSession,
) -> Result<String, String> {
    let url = format!("{MFA_VERIFY_URL}/{}", method.endpoint_name());
    let response = auth_session
        .http()
        .post(url)
        .headers(discord_login_headers(auth_session.fingerprint()))
        .json(&json!({
            "code": code.trim(),
            "login_instance_id": login_instance_id,
            "ticket": ticket,
        }))
        .send()
        .await
        .map_err(|error| format!("No se pudo mandar el código a Discord: {error}"))?;

    let status = response.status();
    let body = response
        .text()
        .await
        .map_err(|error| format!("No se pudo leer la respuesta de Discord: {error}"))?;

    if status.is_success() {
        mfa_token_from_body(&body)
    } else {
        Err(format_login_error(status, &body))
    }
}

fn parse_login_success(body: &str) -> Result<LoginOutcome, String> {
    #[derive(Deserialize)]
    struct LoginResponse {
        token: Option<String>,
        mfa: Option<bool>,
        sms: Option<bool>,
        totp: Option<bool>,
        ticket: Option<String>,
        login_instance_id: Option<String>,
        suspended_user_token: Option<String>,
        #[serde(default)]
        required_actions: Vec<String>,
    }

    let response: LoginResponse =
        serde_json::from_str(body).map_err(|error| format!("No se pudo leer la respuesta de login: {error}"))?;
    if response.suspended_user_token.is_some() {
        return Err(SUSPENDED_ACCOUNT_ERROR.to_owned());
    }
    if !response.required_actions.is_empty() {
        return Ok(LoginOutcome::RequiredActions(response.required_actions));
    }
    if let Some(token) = response.token {
        return Ok(LoginOutcome::Token(token));
    }
    if response.mfa == Some(true) {
        let ticket = response.ticket.ok_or("Discord no mandó el ticket de 2FA")?;
        let login_instance_id = response
            .login_instance_id
            .ok_or("Discord no mandó el login_instance_id de 2FA")?;
        let mut methods = Vec::new();
        if response.totp == Some(true) {
            methods.push(MfaMethod::Totp);
        }
        if response.sms == Some(true) {
            methods.push(MfaMethod::Sms);
        }
        if methods.is_empty() {
            return Err("Discord pide 2FA, pero con un método que este cliente no soporta (solo TOTP y SMS)".into());
        }
        return Ok(LoginOutcome::MfaRequired(MfaChallenge { ticket, login_instance_id, methods }));
    }
    Err("La respuesta de Discord no trajo ni token ni pedido de 2FA".into())
}

fn mfa_token_from_body(body: &str) -> Result<String, String> {
    #[derive(Deserialize)]
    struct MfaVerifyResponse {
        token: Option<String>,
    }

    let response: MfaVerifyResponse =
        serde_json::from_str(body).map_err(|error| format!("No se pudo leer la respuesta de 2FA: {error}"))?;
    response.token.ok_or_else(|| "La respuesta de 2FA no trajo un token".to_owned())
}

fn format_login_error(status: reqwest::StatusCode, body: &str) -> String {
    #[derive(Deserialize)]
    struct DiscordError {
        code: Option<i64>,
        message: Option<String>,
        captcha_key: Option<Value>,
        suspended_user_token: Option<String>,
    }

    let Ok(error) = serde_json::from_str::<DiscordError>(body) else {
        return format!("El login falló (HTTP {status})");
    };
    if error.captcha_key.is_some() {
        return "Discord pide verificación por captcha, así que no se puede completar el login por usuario/contraseña acá. Iniciá sesión por código QR en su lugar.".to_string();
    }
    if error.suspended_user_token.is_some() {
        return SUSPENDED_ACCOUNT_ERROR.to_owned();
    }
    match error.code {
        Some(50035) => "Discord rechazó el usuario/teléfono o la contraseña".to_string(),
        Some(20013) | Some(20011) => "Esta cuenta está deshabilitada o marcada para borrarse".to_string(),
        Some(70007) => "Discord pide verificar el teléfono antes de poder iniciar sesión".to_string(),
        Some(70009) => "Discord pide verificar el email antes de poder iniciar sesión".to_string(),
        _ => error.message.unwrap_or_else(|| format!("El login falló (HTTP {status})")),
    }
}

/// Los números de teléfono se aceptan con espacios/guiones para que sea
/// más fácil de tipear, pero Discord los espera pegados (`+54 9 11...`
/// → `+5491...`).
fn normalize_login_identifier(login: &str) -> String {
    let trimmed = login.trim();
    if trimmed.starts_with('+') {
        trimmed.replace([' ', '-'], "")
    } else {
        trimmed.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::{LoginOutcome, MfaMethod, format_login_error, normalize_login_identifier, parse_login_success};
    use reqwest::StatusCode;

    #[test]
    fn parse_login_success_prioritizes_required_actions_before_token() {
        let outcome = parse_login_success(r#"{"token":"abc"}"#).expect("token should parse");
        assert!(matches!(outcome, LoginOutcome::Token(token) if token == "abc"));

        let outcome = parse_login_success(r#"{"token":"temporary","required_actions":["update_password"]}"#)
            .expect("required actions should parse");
        assert!(matches!(
            outcome,
            LoginOutcome::RequiredActions(actions) if actions == vec!["update_password".to_owned()]
        ));
    }

    #[test]
    fn parse_login_success_returns_supported_mfa_methods() {
        let outcome = parse_login_success(
            r#"{"mfa":true,"sms":true,"totp":true,"ticket":"ticket","login_instance_id":"login"}"#,
        )
        .expect("mfa should parse");

        let LoginOutcome::MfaRequired(challenge) = outcome else {
            panic!("expected MFA challenge");
        };
        assert_eq!(challenge.ticket, "ticket");
        assert_eq!(challenge.login_instance_id, "login");
        assert_eq!(challenge.methods, vec![MfaMethod::Totp, MfaMethod::Sms]);
    }

    #[test]
    fn parse_login_success_reports_suspended_account() {
        let Err(error) = parse_login_success(r#"{"suspended_user_token":"secret"}"#) else {
            panic!("suspended account should not be accepted as a login");
        };
        assert!(error.contains("Safety Hub"));
        assert!(!error.contains("secret"));
    }

    #[test]
    fn phone_login_identifier_removes_common_separators() {
        assert_eq!(normalize_login_identifier("+1 234-567"), "+1234567");
    }

    #[test]
    fn login_errors_are_clear_without_raw_sensitive_body() {
        let cases = [
            (
                StatusCode::BAD_REQUEST,
                r#"{"captcha_key":["captcha-required"],"captcha_rqtoken":"secret"}"#,
                "captcha",
            ),
            (StatusCode::TOO_MANY_REQUESTS, "not json secret", "El login falló (HTTP 429 Too Many Requests)"),
            (StatusCode::FORBIDDEN, r#"{"message":"suspended","suspended_user_token":"secret"}"#, "Safety Hub"),
        ];
        for (status, body, expected) in cases {
            let message = format_login_error(status, body);
            assert!(message.contains(expected));
            assert!(!message.contains("secret"));
        }
    }
}
