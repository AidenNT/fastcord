//! Formulario de ingreso de un servidor (lo que Discord llama "detección de
//! miembros" / "membership screening").
//!
//! Algunos servers no te dejan participar hasta que aceptas sus reglas o
//! contestas unas preguntas. El cliente oficial lo muestra justo después de
//! unirte con una invitación; acá pasa lo mismo:
//!
//! 1. `discord::invites::start_join` termina de unir a la cuenta y pide el
//!    formulario con `UwuRest::get_join_form` (`GET /guilds/{id}/member-verification`).
//!    Si el server no tiene formulario Discord contesta 404 y no se muestra nada.
//! 2. [`JoinForm::from_value`] lo convierte en datos propios y `App` lo dibuja
//!    en `ui::shell::overlay::show_join_form`.
//! 3. Al enviar, [`JoinForm::validate`] revisa lo obligatorio,
//!    [`JoinForm::submit_body`] arma el cuerpo y `UwuRest::submit_join_form`
//!    hace el `PUT /guilds/{id}/requests/@me`.
//!
//! El parseo es tolerante: lo que falte o venga con un tipo raro queda vacío en
//! vez de tirar abajo todo el formulario. De cada campo se guarda además el
//! JSON original (`raw`), así al responder se devuelve tal cual lo mandó
//! Discord (con sus claves que acá no se usan) y solo se pisa `response`.

use serde_json::{Value, json};

/// Largo máximo de lo que se escribe en una pregunta abierta.
pub const MAX_ANSWER_LEN: usize = 1000;

/// Tipo de una pregunta del formulario (`field_type`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FieldKind {
    /// Reglas del servidor + "acepto" (`TERMS`). Las reglas vienen en `values`.
    Terms,
    /// Respuesta corta (`TEXT_INPUT`).
    ShortText,
    /// Respuesta larga (`PARAGRAPH`).
    Paragraph,
    /// Opción múltiple (`MULTIPLE_CHOICE`). Las opciones vienen en `values`.
    Choice,
    /// Un tipo que este cliente todavía no sabe mostrar.
    Unsupported(String),
}

impl FieldKind {
    fn from_wire(name: &str) -> Self {
        match name {
            "TERMS" => FieldKind::Terms,
            "TEXT_INPUT" => FieldKind::ShortText,
            "PARAGRAPH" => FieldKind::Paragraph,
            "MULTIPLE_CHOICE" => FieldKind::Choice,
            other => FieldKind::Unsupported(other.to_owned()),
        }
    }
}

/// Una pregunta del formulario junto con lo que la persona fue contestando.
#[derive(Clone, Debug)]
pub struct JoinField {
    pub kind: FieldKind,
    pub label: String,
    pub description: Option<String>,
    pub required: bool,
    /// Reglas (`Terms`) u opciones (`Choice`).
    pub values: Vec<String>,
    /// Respuesta de las preguntas abiertas.
    pub text: String,
    /// "Acepto" de las reglas.
    pub accepted: bool,
    /// Opción elegida (índice en `values`).
    pub choice: Option<usize>,
    /// El campo tal como lo mandó Discord.
    raw: Value,
}

/// Una opción o regla: o un string, o un objeto con el texto bajo alguna de
/// las claves habituales (`label`, `name`, `value`, `text`, `title`).
fn value_text(item: &Value) -> Option<String> {
    match item {
        Value::String(s) => Some(s.clone()),
        Value::Object(map) => ["label", "name", "value", "text", "title"]
            .iter()
            .find_map(|key| map.get(*key).and_then(Value::as_str))
            .map(str::to_owned),
        _ => None,
    }
}

/// Las reglas / opciones de un campo. Discord las manda en `values`, pero se
/// aceptan también `choices` u `options`, y que cada una sea un string o un
/// objeto. Lo que no se reconoce se salta en vez de vaciar la lista entera.
fn parse_values(value: &Value) -> Vec<String> {
    ["values", "choices", "options"]
        .iter()
        .filter_map(|key| value.get(*key).and_then(Value::as_array))
        .map(|items| items.iter().filter_map(value_text).collect::<Vec<_>>())
        .find(|list| !list.is_empty())
        .unwrap_or_default()
}

impl JoinField {
    fn from_value(value: &Value) -> Self {
        let text_of = |key: &str| {
            value
                .get(key)
                .and_then(Value::as_str)
                .map(str::to_owned)
                .filter(|t| !t.trim().is_empty())
        };
        let kind = FieldKind::from_wire(value.get("field_type").and_then(Value::as_str).unwrap_or(""));
        let required = value
            .get("required")
            .and_then(Value::as_bool)
            // Las reglas son siempre de aceptación obligatoria.
            .unwrap_or(kind == FieldKind::Terms);
        let values = parse_values(value);
        if kind == FieldKind::Choice && values.is_empty() {
            log::warn!("Pregunta de opción múltiple sin opciones reconocibles: {value}");
        }
        Self {
            kind,
            label: text_of("label").unwrap_or_default(),
            description: text_of("description"),
            required,
            values,
            text: String::new(),
            accepted: false,
            choice: None,
            raw: value.clone(),
        }
    }

    /// Texto con el que se nombra el campo en los avisos de validación.
    fn name(&self) -> String {
        match self.kind {
            FieldKind::Terms => "las reglas del servidor".to_owned(),
            _ if self.label.is_empty() => "una pregunta".to_owned(),
            _ => format!("«{}»", self.label),
        }
    }

    /// El campo con su `response`, listo para mandar a Discord.
    fn submit_value(&self) -> Value {
        let response = match self.kind {
            FieldKind::Terms => Value::Bool(self.accepted),
            FieldKind::ShortText | FieldKind::Paragraph => Value::String(self.text.trim().to_owned()),
            FieldKind::Choice => self.choice.map(|i| json!(i)).unwrap_or(Value::Null),
            // No se toca lo que vino.
            FieldKind::Unsupported(_) => self.raw.get("response").cloned().unwrap_or(Value::Null),
        };
        let mut out = if self.raw.is_object() {
            self.raw.clone()
        } else {
            json!({})
        };
        out["response"] = response;
        out
    }
}

/// El formulario completo de un server.
#[derive(Clone, Debug)]
pub struct JoinForm {
    pub guild_id: String,
    pub guild_name: String,
    /// Ícono del server ya como URL del CDN (el de la invitación).
    pub guild_icon_url: Option<String>,
    /// Versión del formulario: Discord la pide de vuelta al enviar.
    pub version: String,
    /// Texto de presentación que puso el server (opcional).
    pub description: Option<String>,
    pub fields: Vec<JoinField>,
}

/// Cómo quedó el ingreso después de enviar el formulario.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum JoinOutcome {
    /// Ya puedes participar.
    Approved,
    /// Un moderador tiene que revisar las respuestas.
    PendingReview,
    Rejected,
    /// Discord no dijo el estado (se trata como enviado).
    Unknown,
}

impl JoinOutcome {
    /// Lee `application_status` de la respuesta del `PUT .../requests/@me`.
    pub fn from_response(value: &Value) -> Self {
        match value.get("application_status").and_then(Value::as_str) {
            Some("APPROVED") => JoinOutcome::Approved,
            Some("SUBMITTED") | Some("PENDING") => JoinOutcome::PendingReview,
            Some("REJECTED") => JoinOutcome::Rejected,
            _ => JoinOutcome::Unknown,
        }
    }
}

/// Una solicitud de ingreso a un server (`guild_join_requests` del `READY` y
/// los eventos `GUILD_JOIN_REQUEST_*`): lo que mandaste con el formulario y
/// espera (o ya tuvo) la decisión de los moderadores.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct JoinRequest {
    pub id: String,
    pub guild_id: String,
    /// Nombre del server si el payload lo trae (si no, se busca en los
    /// servers de la cuenta).
    pub guild_name: Option<String>,
    /// Hash del ícono del server si el payload lo trae.
    pub guild_icon: Option<String>,
    pub status: JoinRequestStatus,
    pub rejection_reason: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum JoinRequestStatus {
    /// Enviada, esperando revisión (`SUBMITTED` / `PENDING`).
    Pending,
    Approved,
    Rejected,
    Other,
}

impl JoinRequest {
    /// Lee una solicitud; `None` si no trae ni `guild_id` ni nada útil.
    pub fn from_value(value: &Value) -> Option<Self> {
        let text = |v: Option<&Value>| match v {
            Some(Value::String(s)) if !s.is_empty() => Some(s.clone()),
            Some(Value::Number(n)) => Some(n.to_string()),
            _ => None,
        };
        let guild = value.get("guild");
        let guild_id = text(value.get("guild_id")).or_else(|| text(guild.and_then(|g| g.get("id"))))?;
        let status = match value.get("application_status").and_then(Value::as_str) {
            Some("SUBMITTED") | Some("PENDING") => JoinRequestStatus::Pending,
            Some("APPROVED") => JoinRequestStatus::Approved,
            Some("REJECTED") => JoinRequestStatus::Rejected,
            _ => JoinRequestStatus::Other,
        };
        Some(Self {
            id: text(value.get("id")).unwrap_or_else(|| guild_id.clone()),
            guild_id,
            guild_name: text(guild.and_then(|g| g.get("name"))),
            guild_icon: text(guild.and_then(|g| g.get("icon"))),
            status,
            rejection_reason: text(value.get("rejection_reason")),
        })
    }

    pub fn is_pending(&self) -> bool {
        self.status == JoinRequestStatus::Pending
    }
}

/// Las solicitudes de `READY.guild_join_requests` (un array, o
/// `{ "entries": [...] }`). Lo que no se reconoce se salta.
pub fn parse_join_requests(raw: &Value) -> Vec<JoinRequest> {
    let list = raw.as_array().or_else(|| raw.get("entries").and_then(Value::as_array));
    list.map(|items| items.iter().filter_map(JoinRequest::from_value).collect())
        .unwrap_or_default()
}

impl JoinForm {
    /// Arma el formulario desde la respuesta de `GET .../member-verification`.
    /// Devuelve `None` si no trae ninguna pregunta (no hay nada que mostrar).
    pub fn from_value(value: &Value, guild_id: &str, guild_name: &str, guild_icon_url: Option<String>) -> Option<Self> {
        let fields: Vec<JoinField> = value
            .get("form_fields")
            .and_then(Value::as_array)?
            .iter()
            .map(JoinField::from_value)
            .collect();
        if fields.is_empty() {
            return None;
        }
        Some(Self {
            guild_id: guild_id.to_owned(),
            guild_name: guild_name.to_owned(),
            guild_icon_url,
            version: value
                .get("version")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned(),
            description: value
                .get("description")
                .and_then(Value::as_str)
                .map(str::to_owned)
                .filter(|d| !d.trim().is_empty()),
            fields,
        })
    }

    /// Revisa lo obligatorio. El `Err` es el aviso que se muestra en el
    /// propio formulario.
    pub fn validate(&self) -> Result<(), String> {
        for field in &self.fields {
            let name = field.name();
            match &field.kind {
                FieldKind::Terms if field.required && !field.accepted => {
                    return Err("Tienes que aceptar las reglas del servidor para entrar.".to_owned());
                }
                FieldKind::ShortText | FieldKind::Paragraph if field.required && field.text.trim().is_empty() => {
                    return Err(format!("Falta responder {name}."));
                }
                FieldKind::Choice if field.required && field.choice.is_none() => {
                    return Err(format!("Falta elegir una opción en {name}."));
                }
                FieldKind::Unsupported(_) if field.required => {
                    return Err(format!(
                        "Este formulario tiene {name}, que ecord todavía no puede mostrar. \
                         Complétalo desde el cliente oficial de Discord."
                    ));
                }
                _ => {}
            }
        }
        Ok(())
    }

    /// Cuerpo del `PUT /guilds/{id}/requests/@me`.
    pub fn submit_body(&self) -> Value {
        json!({
            "version": self.version,
            "form_fields": self.fields.iter().map(JoinField::submit_value).collect::<Vec<_>>(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Value {
        json!({
            "version": "2024-01-01T00:00:00+00:00",
            "description": "Bienvenido",
            "form_fields": [
                {
                    "field_type": "TERMS",
                    "label": "Read and agree to the server rules",
                    "description": null,
                    "required": true,
                    "values": ["Sé amable", "Nada de spam"],
                    "response": null
                },
                {
                    "field_type": "PARAGRAPH",
                    "label": "¿Por qué quieres unirte?",
                    "required": true,
                    "values": null,
                    "response": null
                },
                {
                    "field_type": "MULTIPLE_CHOICE",
                    "label": "¿De dónde vienes?",
                    "required": false,
                    "values": ["Amigos", "Búsqueda"],
                    "response": null
                }
            ]
        })
    }

    #[test]
    fn parses_fields_and_kinds() {
        let form = JoinForm::from_value(&sample(), "1", "Mi server", None).unwrap();
        assert_eq!(form.fields.len(), 3);
        assert_eq!(form.fields[0].kind, FieldKind::Terms);
        assert_eq!(form.fields[0].values, vec!["Sé amable", "Nada de spam"]);
        assert_eq!(form.fields[1].kind, FieldKind::Paragraph);
        assert!(form.fields[1].values.is_empty());
        assert_eq!(form.fields[2].kind, FieldKind::Choice);
        assert!(!form.fields[2].required);
        assert_eq!(form.description.as_deref(), Some("Bienvenido"));
    }

    #[test]
    fn no_fields_means_no_form() {
        assert!(JoinForm::from_value(&json!({ "form_fields": [] }), "1", "x", None).is_none());
        assert!(JoinForm::from_value(&json!({}), "1", "x", None).is_none());
    }

    #[test]
    fn validation_asks_for_required_answers() {
        let mut form = JoinForm::from_value(&sample(), "1", "Mi server", None).unwrap();
        assert!(form.validate().is_err());
        form.fields[0].accepted = true;
        assert!(form.validate().is_err());
        form.fields[1].text = "   ".to_owned();
        assert!(form.validate().is_err());
        form.fields[1].text = "Me invitaron".to_owned();
        assert!(form.validate().is_ok());
    }

    #[test]
    fn required_unsupported_field_blocks_submit() {
        let value = json!({
            "version": "v",
            "form_fields": [{ "field_type": "ALGO_NUEVO", "label": "X", "required": true }]
        });
        let form = JoinForm::from_value(&value, "1", "x", None).unwrap();
        assert!(form.validate().is_err());
    }

    #[test]
    fn submit_body_fills_responses_and_keeps_the_rest() {
        let mut form = JoinForm::from_value(&sample(), "1", "Mi server", None).unwrap();
        form.fields[0].accepted = true;
        form.fields[1].text = "  Me invitaron  ".to_owned();
        form.fields[2].choice = Some(1);
        let body = form.submit_body();
        assert_eq!(body["version"], "2024-01-01T00:00:00+00:00");
        let sent = body["form_fields"].as_array().unwrap();
        assert_eq!(sent[0]["response"], json!(true));
        assert_eq!(sent[0]["field_type"], "TERMS");
        assert_eq!(sent[0]["values"], json!(["Sé amable", "Nada de spam"]));
        assert_eq!(sent[1]["response"], json!("Me invitaron"));
        assert_eq!(sent[2]["response"], json!(1));
    }

    #[test]
    fn outcome_from_status() {
        assert_eq!(
            JoinOutcome::from_response(&json!({"application_status": "APPROVED"})),
            JoinOutcome::Approved
        );
        assert_eq!(
            JoinOutcome::from_response(&json!({"application_status": "SUBMITTED"})),
            JoinOutcome::PendingReview
        );
        assert_eq!(
            JoinOutcome::from_response(&json!({"application_status": "REJECTED"})),
            JoinOutcome::Rejected
        );
        assert_eq!(JoinOutcome::from_response(&json!({})), JoinOutcome::Unknown);
    }

    #[test]
    fn choice_options_accept_objects_and_other_keys() {
        let value = json!({
            "version": "v",
            "form_fields": [
                { "field_type": "MULTIPLE_CHOICE", "label": "A", "values": [{"label": "Uno"}, {"value": "Dos"}, 3] },
                { "field_type": "MULTIPLE_CHOICE", "label": "B", "values": null, "choices": ["X", "Y"] }
            ]
        });
        let form = JoinForm::from_value(&value, "1", "x", None).unwrap();
        assert_eq!(form.fields[0].values, vec!["Uno", "Dos"]);
        assert_eq!(form.fields[1].values, vec!["X", "Y"]);
    }

    #[test]
    fn parses_join_requests_from_ready() {
        let raw = json!([
            { "id": "9", "guild_id": "1", "application_status": "SUBMITTED", "guild": { "name": "Mi server", "icon": "abc" } },
            { "id": "10", "guild_id": "2", "application_status": "REJECTED", "rejection_reason": "No" },
            { "application_status": "SUBMITTED" }
        ]);
        let list = parse_join_requests(&raw);
        assert_eq!(list.len(), 2);
        assert!(list[0].is_pending());
        assert_eq!(list[0].guild_name.as_deref(), Some("Mi server"));
        assert_eq!(list[1].status, JoinRequestStatus::Rejected);
        assert_eq!(list[1].rejection_reason.as_deref(), Some("No"));
        assert!(parse_join_requests(&json!({})).is_empty());
    }
}
