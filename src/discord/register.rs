//! Registro de una cuenta nueva desde el propio cliente
//! (`POST https://discord.com/api/v9/auth/register`), como alternativa a
//! abrir `discord.com/register` en el navegador.
//!
//! Sigue el mismo patrón que `password_auth` (hilo de SO propio + runtime de
//! tokio propio, resultado por `std::sync::mpsc::Sender<AppEvent>`) y reusa su
//! bucle de captcha (`password_auth::post_json_with_headers`): si Discord
//! contesta 400 con un hCaptcha, se abre la ventana de siempre
//! (`discord::captcha`), la persona lo resuelve y el pedido se repite solo con
//! `X-Captcha-Key` / `X-Captcha-Rqtoken` / `X-Captcha-Session-Id`.
//!
//! Headers: salen de `auth_http::discord_register_headers` —
//! `X-Discord-Locale`, `X-Discord-Timezone`, `X-Super-Properties`,
//! `X-Installation-ID`, `X-Fingerprint` y `X-Debug-Options`.
//!
//! Decisiones a propósito:
//! * Un registro por vez y siempre a pedido de la persona: no hay reintentos
//!   automáticos ni forma de encadenar varios registros, y el captcha lo
//!   resuelve ella (no se automatiza).
//! * `consent` y `promotional_email_opt_in` los define lo que la persona
//!   marcó en el formulario; no se mandan fijos.
//! * La fecha de nacimiento es la real que se escribe (con la edad mínima de
//!   13 años de Discord comprobada acá para dar un error claro antes de
//!   mandar nada; igual la valida Discord).
//! * Si el registro sale bien, Discord devuelve un token de sesión y se
//!   reusa `AppEvent::PasswordLoginToken`: desde ahí es el mismo camino que
//!   un login por contraseña (guardar cuenta, conectar al Gateway).
//!
//! ⚠️ Igual que `password_auth`: usar la API de usuario con un cliente que no
//! es el oficial va contra los términos de Discord, y esto no se pudo probar
//! contra los servidores reales desde donde se escribió — tratalo como primer
//! borrador.

use chrono::{Datelike, NaiveDate};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::discord::AppEvent;
use crate::discord::auth_http::{DiscordAuthSession, discord_register_headers};
use crate::discord::password_auth::post_json_with_headers;

const REGISTER_URL: &str = "https://discord.com/api/v9/auth/register";

/// Edad mínima para tener cuenta de Discord (13 en casi todo el mundo).
pub const MIN_AGE_YEARS: i32 = 13;

/// Lo que se manda a `/auth/register`, ya validado y normalizado.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RegisterRequest {
    pub email: String,
    /// Siempre en minúsculas (Discord ya no admite mayúsculas).
    pub username: String,
    /// Nombre para mostrar; vacío = sin nombre para mostrar (`null`).
    pub global_name: String,
    pub password: String,
    /// `AAAA-MM-DD`.
    pub date_of_birth: String,
    /// Solo `true` si la persona aceptó los Términos y la Política de
    /// Privacidad (el formulario no deja mandar sin eso).
    pub consent: bool,
    pub promotional_email_opt_in: bool,
}

/// Arranca el registro en un hilo de fondo. El resultado llega por `tx`:
/// `AppEvent::PasswordLoginToken` si salió bien, `AppEvent::RegisterFailed`
/// si no (ya con el mensaje listo para mostrar).
pub fn spawn(request: RegisterRequest, tx: std::sync::mpsc::Sender<AppEvent>) {
    std::thread::spawn(move || {
        let Ok(rt) = tokio::runtime::Builder::new_current_thread().enable_all().build() else {
            let _ = tx.send(AppEvent::RegisterFailed("No se pudo iniciar el runtime async".into()));
            return;
        };
        rt.block_on(async move {
            let fingerprint = crate::discord::init_fingerprint().await;
            let auth_session = DiscordAuthSession::new(fingerprint);
            match register(&request, &auth_session).await {
                Ok(token) => {
                    let _ = tx.send(AppEvent::PasswordLoginToken(token));
                }
                Err(error) => {
                    let _ = tx.send(AppEvent::RegisterFailed(error));
                }
            }
        });
    });
}

async fn register(request: &RegisterRequest, auth_session: &DiscordAuthSession) -> Result<String, String> {
    let body = build_body(request, auth_session.fingerprint().anonymous_fingerprint());
    let (status, text) = post_json_with_headers(
        auth_session,
        REGISTER_URL,
        body,
        "No se pudo mandar el registro a Discord",
        "register (crear cuenta)",
        discord_register_headers,
    )
    .await?;

    if status.is_success() {
        parse_register_success(&text)
    } else {
        Err(format_register_error(status, &text))
    }
}

/// Cuerpo de `/auth/register`. `fingerprint` repite el `X-Fingerprint`
/// (`null` si todavía no se consiguió uno).
fn build_body(request: &RegisterRequest, fingerprint: Option<&str>) -> Value {
    let global_name = match request.global_name.trim() {
        "" => Value::Null,
        name => Value::String(name.to_owned()),
    };
    json!({
        "fingerprint": fingerprint,
        "email": request.email,
        "username": request.username,
        "global_name": global_name,
        "password": request.password,
        "invite": Value::Null,
        "consent": request.consent,
        "date_of_birth": request.date_of_birth,
        "gift_code_sku_id": Value::Null,
        "promotional_email_opt_in": request.promotional_email_opt_in,
    })
}

fn parse_register_success(body: &str) -> Result<String, String> {
    #[derive(Deserialize)]
    struct RegisterResponse {
        token: Option<String>,
    }

    let response: RegisterResponse = serde_json::from_str(body)
        .map_err(|error| format!("No se pudo leer la respuesta del registro: {error}"))?;
    response
        .token
        .filter(|token| !token.is_empty())
        .ok_or_else(|| "Discord creó la respuesta pero no mandó un token de sesión".to_owned())
}

/// Convierte el error de Discord en un mensaje para mostrar. Nunca incluye el
/// cuerpo crudo (puede traer tokens de captcha ni datos sensibles).
fn format_register_error(status: reqwest::StatusCode, body: &str) -> String {
    #[derive(Deserialize)]
    struct DiscordError {
        message: Option<String>,
        errors: Option<Value>,
        captcha_key: Option<Value>,
        retry_after: Option<f64>,
    }

    let Ok(error) = serde_json::from_str::<DiscordError>(body) else {
        return format!("El registro falló (HTTP {status})");
    };
    if status == reqwest::StatusCode::TOO_MANY_REQUESTS {
        return match error.retry_after {
            Some(seconds) => format!(
                "Discord pide esperar antes de intentar de nuevo ({} s). Probá más tarde.",
                seconds.ceil() as u64
            ),
            None => "Discord pide esperar antes de intentar de nuevo. Probá más tarde.".to_owned(),
        };
    }
    if error.captcha_key.is_some() {
        return "Discord no aceptó la verificación por captcha (o pidió una que no se puede mostrar). Probá de nuevo.".to_owned();
    }
    if let Some(errors) = &error.errors {
        let mut lines = Vec::new();
        collect_field_errors(errors, "", &mut lines);
        if !lines.is_empty() {
            return lines.join("\n");
        }
    }
    error
        .message
        .unwrap_or_else(|| format!("El registro falló (HTTP {status})"))
}

/// Recorre el árbol `errors` de Discord (`{"email": {"_errors": [{code,
/// message}]}}`, anidado a veces) y arma una línea por problema.
fn collect_field_errors(node: &Value, field: &str, out: &mut Vec<String>) {
    let Some(map) = node.as_object() else { return };
    if let Some(Value::Array(items)) = map.get("_errors") {
        for item in items {
            let code = item.get("code").and_then(Value::as_str).unwrap_or_default();
            let message = item.get("message").and_then(Value::as_str).unwrap_or_default();
            out.push(describe_field_error(field, code, message));
        }
    }
    for (key, child) in map {
        if key != "_errors" {
            let path = if field.is_empty() { key.clone() } else { field.to_owned() };
            collect_field_errors(child, &path, out);
        }
    }
}

fn field_label(field: &str) -> &str {
    match field {
        "email" => "Correo electrónico",
        "username" => "Nombre de usuario",
        "global_name" => "Nombre para mostrar",
        "password" => "Contraseña",
        "date_of_birth" => "Fecha de nacimiento",
        "consent" => "Términos de servicio",
        "" => "Registro",
        other => other,
    }
}

fn describe_field_error(field: &str, code: &str, message: &str) -> String {
    let label = field_label(field);
    let spanish = match code {
        "EMAIL_ALREADY_REGISTERED" => Some("ya hay una cuenta con este correo"),
        "USERNAME_ALREADY_TAKEN" => Some("ese nombre de usuario ya está en uso"),
        "BASE_TYPE_REQUIRED" => Some("es obligatorio"),
        _ => None,
    };
    match (spanish, message.is_empty()) {
        (Some(text), _) => format!("{label}: {text}"),
        (None, false) => format!("{label}: {message}"),
        (None, true) => format!("{label}: dato no válido"),
    }
}

/// Valida día/mes/año tipeados y devuelve `AAAA-MM-DD`, comprobando que la
/// fecha exista, no sea futura y que la persona tenga al menos
/// [`MIN_AGE_YEARS`] años a la fecha `today`.
pub fn validate_birth_date(day: &str, month: &str, year: &str, today: NaiveDate) -> Result<String, String> {
    let parse = |text: &str| text.trim().parse::<u32>().ok();
    let (Some(day), Some(month), Some(year)) = (parse(day), parse(month), parse(year)) else {
        return Err("Completá la fecha de nacimiento (día, mes y año con números)".to_owned());
    };
    let Some(birth) = NaiveDate::from_ymd_opt(year as i32, month, day) else {
        return Err("La fecha de nacimiento no existe".to_owned());
    };
    if birth > today || year < 1900 {
        return Err("La fecha de nacimiento no es válida".to_owned());
    }
    let had_birthday = (today.month(), today.day()) >= (birth.month(), birth.day());
    let age = today.year() - birth.year() - if had_birthday { 0 } else { 1 };
    if age < MIN_AGE_YEARS {
        return Err(format!(
            "Hay que tener al menos {MIN_AGE_YEARS} años para crear una cuenta de Discord"
        ));
    }
    Ok(format!("{:04}-{:02}-{:02}", birth.year(), birth.month(), birth.day()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use reqwest::StatusCode;

    fn request() -> RegisterRequest {
        RegisterRequest {
            email: "a@b.com".into(),
            username: "alguien".into(),
            global_name: "  ".into(),
            password: "correcta-caballo".into(),
            date_of_birth: "2000-01-01".into(),
            consent: true,
            promotional_email_opt_in: false,
        }
    }

    fn day(y: i32, m: u32, d: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, d).unwrap()
    }

    #[test]
    fn body_has_the_exact_shape_discord_expects() {
        let body = build_body(&request(), Some("fp-123"));
        assert_eq!(
            body,
            json!({
                "fingerprint": "fp-123",
                "email": "a@b.com",
                "username": "alguien",
                "global_name": null,
                "password": "correcta-caballo",
                "invite": null,
                "consent": true,
                "date_of_birth": "2000-01-01",
                "gift_code_sku_id": null,
                "promotional_email_opt_in": false,
            })
        );
        assert_eq!(build_body(&request(), None)["fingerprint"], Value::Null);
    }

    #[test]
    fn display_name_is_sent_trimmed_when_present() {
        let mut req = request();
        req.global_name = "  Ana  ".into();
        assert_eq!(build_body(&req, None)["global_name"], json!("Ana"));
    }

    #[test]
    fn success_needs_a_token() {
        assert_eq!(parse_register_success(r#"{"token":"abc"}"#), Ok("abc".to_owned()));
        assert!(parse_register_success(r#"{}"#).is_err());
        assert!(parse_register_success(r#"{"token":""}"#).is_err());
        assert!(parse_register_success("no json").is_err());
    }

    #[test]
    fn field_errors_are_listed_per_field() {
        let body = r#"{"code":50035,"message":"Invalid Form Body","errors":{
            "email":{"_errors":[{"code":"EMAIL_ALREADY_REGISTERED","message":"x"}]},
            "password":{"_errors":[{"code":"BASE_TYPE_BAD_LENGTH","message":"Debe tener entre 8 y 128 caracteres."}]}
        }}"#;
        let message = format_register_error(StatusCode::BAD_REQUEST, body);
        assert!(message.contains("Correo electrónico: ya hay una cuenta con este correo"));
        assert!(message.contains("Contraseña: Debe tener entre 8 y 128 caracteres."));
    }

    #[test]
    fn rate_limit_shows_the_wait_without_raw_body() {
        let message = format_register_error(StatusCode::TOO_MANY_REQUESTS, r#"{"retry_after":12.3,"secret":"x"}"#);
        assert!(message.contains("13 s"));
        assert!(!message.contains("secret"));
    }

    #[test]
    fn unparseable_errors_do_not_leak_the_body() {
        let message = format_register_error(StatusCode::INTERNAL_SERVER_ERROR, "token=secret");
        assert!(message.contains("500"));
        assert!(!message.contains("secret"));
    }

    #[test]
    fn birth_date_rejects_impossible_future_and_underage_dates() {
        let today = day(2026, 10, 23);
        assert_eq!(validate_birth_date("1", "1", "2000", today), Ok("2000-01-01".into()));
        assert_eq!(validate_birth_date(" 5 ", "03", "1995", today), Ok("1995-03-05".into()));
        assert!(validate_birth_date("31", "2", "2000", today).is_err());
        assert!(validate_birth_date("x", "2", "2000", today).is_err());
        assert!(validate_birth_date("1", "1", "2030", today).is_err());
        assert!(validate_birth_date("1", "1", "1850", today).is_err());
    }

    #[test]
    fn minimum_age_counts_the_birthday() {
        let today = day(2026, 10, 23);
        // Cumple 13 hoy: alcanza. Mañana: todavía no.
        assert!(validate_birth_date("23", "10", "2013", today).is_ok());
        assert!(validate_birth_date("24", "10", "2013", today).is_err());
    }
}
