//! Slash commands (comandos de aplicación `CHAT_INPUT`): modelos, catálogo y
//! armado de la interacción. Sin UI ni red: lo que habla con Discord vive en
//! `uwu_rest` y lo que se dibuja en `ui::slash`.
//!
//! Flujo completo:
//! 1. `GET /guilds/{id}/application-command-index` (o `/channels/{id}/...`
//!    en un DM) devuelve los comandos que ESTA cuenta puede usar ahí.
//! 2. [`build_catalog`] los aplana: un comando con subcomandos pasa a ser una
//!    entrada por cada hoja (`/permisos usuario ver`), que es como los lista
//!    el cliente oficial.
//! 3. La UI deja elegir uno y completar sus opciones; [`build_options`] las
//!    convierte a los tipos que pide la API.
//! 4. [`interaction_body`] arma el `POST /interactions` (tipo 2). La respuesta
//!    de la app llega después por el Gateway como un mensaje normal.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

/// Tipos de opción (`ApplicationCommandOptionType`).
pub mod opt {
    pub const SUB_COMMAND: u8 = 1;
    pub const SUB_COMMAND_GROUP: u8 = 2;
    pub const STRING: u8 = 3;
    pub const INTEGER: u8 = 4;
    pub const BOOLEAN: u8 = 5;
    pub const USER: u8 = 6;
    pub const CHANNEL: u8 = 7;
    pub const ROLE: u8 = 8;
    pub const MENTIONABLE: u8 = 9;
    pub const NUMBER: u8 = 10;
    pub const ATTACHMENT: u8 = 11;
}

/// `CHAT_INPUT`: los comandos que se escriben con `/`.
const CHAT_INPUT: u8 = 1;

fn is_false(value: &bool) -> bool {
    !*value
}

fn chat_input() -> u8 {
    CHAT_INPUT
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct CommandChoice {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub value: Value,
}

impl CommandChoice {
    /// El valor de la opción como texto (los números sin comillas).
    pub fn value_text(&self) -> String {
        match &self.value {
            Value::String(s) => s.clone(),
            other => other.to_string(),
        }
    }
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct CommandOption {
    #[serde(rename = "type", default)]
    pub kind: u8,
    #[serde(default)]
    pub name: String,
    /// Igual que en el comando: si la opción viene traducida, `name` trae la
    /// traducción y el nombre original viene acá. La interacción tiene que
    /// llevar el original o Discord responde `APPLICATION_COMMAND_OPTION_INVALID`.
    #[serde(default, skip_serializing)]
    pub name_default: Option<String>,
    #[serde(default)]
    pub description: String,
    #[serde(default, skip_serializing)]
    pub description_default: Option<String>,
    #[serde(default, skip_serializing_if = "is_false")]
    pub required: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub choices: Vec<CommandChoice>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub options: Vec<CommandOption>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min_value: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_value: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min_length: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_length: Option<u32>,
    #[serde(default, skip_serializing_if = "is_false")]
    pub autocomplete: bool,
}

impl CommandOption {
    /// Nombre ORIGINAL (sin traducir), el que espera la API.
    pub fn api_name(&self) -> &str {
        self.name_default.as_deref().unwrap_or(&self.name)
    }

    /// Copia (recursiva) con nombres y descripciones originales.
    fn for_api(&self) -> CommandOption {
        let mut out = self.clone();
        out.name = self.api_name().to_string();
        if let Some(description) = &self.description_default {
            out.description = description.clone();
        }
        out.options = self.options.iter().map(CommandOption::for_api).collect();
        out
    }
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct AppCommand {
    #[serde(default)]
    pub id: String,
    #[serde(rename = "type", default = "chat_input")]
    pub kind: u8,
    #[serde(default)]
    pub application_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub guild_id: Option<String>,
    #[serde(default)]
    pub name: String,
    /// En el índice, si el comando está traducido, `name` trae la traducción y
    /// el nombre original viene acá. La interacción tiene que llevar el
    /// original.
    #[serde(default, skip_serializing)]
    pub name_default: Option<String>,
    #[serde(default)]
    pub description: String,
    #[serde(default, skip_serializing)]
    pub description_default: Option<String>,
    #[serde(default)]
    pub options: Vec<CommandOption>,
    #[serde(default)]
    pub version: String,
    #[serde(default)]
    pub default_member_permissions: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dm_permission: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub contexts: Option<Vec<u8>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub integration_types: Option<Vec<u8>>,
    #[serde(default)]
    pub nsfw: bool,
}

impl AppCommand {
    /// Copia con el nombre y la descripción ORIGINALES (sin traducir), que es
    /// lo que Discord espera dentro de `application_command`.
    fn for_api(&self) -> AppCommand {
        let mut out = self.clone();
        if let Some(name) = &self.name_default {
            out.name = name.clone();
        }
        if let Some(description) = &self.description_default {
            out.description = description.clone();
        }
        out.options = self.options.iter().map(CommandOption::for_api).collect();
        out
    }

    fn api_name(&self) -> &str {
        self.name_default.as_deref().unwrap_or(&self.name)
    }
}

#[derive(Clone, Debug, Default)]
pub struct IndexApplication {
    pub id: String,
    pub name: String,
    /// Ícono de la app (o avatar de su bot) ya como URL del CDN.
    pub icon_url: Option<String>,
}

/// Respuesta de `application-command-index`.
#[derive(Clone, Debug, Default)]
pub struct CommandIndex {
    pub applications: Vec<IndexApplication>,
    pub commands: Vec<AppCommand>,
}

/// Lee la respuesta del índice. Un comando que no se pueda leer se descarta
/// solo, sin tirar abajo el resto (cada app trae el suyo y Discord agrega
/// campos todo el tiempo).
pub fn parse_index(value: &Value) -> CommandIndex {
    let applications: Vec<IndexApplication> = value
        .get("applications")
        .and_then(Value::as_array)
        .map(|apps| {
            apps.iter()
                .filter_map(|app| {
                    let id = app.get("id")?.as_str()?.to_string();
                    let text = |key: &str| {
                        app.get(key).and_then(Value::as_str).filter(|s| !s.trim().is_empty()).map(str::to_string)
                    };
                    let icon_url = text("icon")
                        .map(|hash| format!("https://cdn.discordapp.com/app-icons/{id}/{hash}.png?size=64"))
                        .or_else(|| {
                            // Sin ícono propio, el avatar del usuario bot.
                            let bot = app.get("bot")?;
                            let bot_id = bot.get("id")?.as_str()?;
                            let hash = bot.get("avatar")?.as_str().filter(|s| !s.is_empty())?;
                            let ext = if hash.starts_with("a_") { "gif" } else { "png" };
                            Some(format!("https://cdn.discordapp.com/avatars/{bot_id}/{hash}.{ext}?size=64"))
                        });
                    Some(IndexApplication {
                        name: app.get("name").and_then(Value::as_str).unwrap_or_default().to_string(),
                        icon_url,
                        id,
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    let commands: Vec<AppCommand> = value
        .get("application_commands")
        .and_then(Value::as_array)
        .map(|commands| {
            commands
                .iter()
                .filter_map(|raw| match serde_json::from_value::<AppCommand>(raw.clone()) {
                    Ok(command) => Some(command),
                    Err(e) => {
                        log::debug!("Comando de aplicación ilegible, se omite: {e}");
                        None
                    }
                })
                .collect()
        })
        .unwrap_or_default();
    CommandIndex { applications, commands }
}

// ---------------------------------------------------------------------
// Catálogo aplanado
// ---------------------------------------------------------------------

/// Una "hoja" ejecutable: un comando sin subcomandos o un subcomando.
#[derive(Clone, Debug)]
pub struct SlashEntry {
    /// El comando raíz (es lo que viaja en `application_command`).
    pub command: Arc<AppCommand>,
    /// `["permisos", "usuario", "ver"]`: comando, grupo (si hay) y subcomando.
    pub path: Vec<String>,
    /// Lo mismo que `path` pero con los nombres ORIGINALES (sin traducir):
    /// es lo que viaja en `data.options`.
    pub api_path: Vec<String>,
    /// Opciones de la hoja (sus argumentos), con las obligatorias primero.
    pub args: Vec<CommandOption>,
    pub description: String,
    pub app_name: String,
    /// Id de la app dueña (agrupa los comandos por app en el menú).
    pub app_id: String,
    /// Ícono de la app, si tiene.
    pub app_icon_url: Option<String>,
    /// `path` unido con espacios: lo que se muestra y contra lo que se busca.
    pub display: String,
}

#[derive(Clone, Debug, Default)]
pub struct SlashCatalog {
    pub entries: Vec<Arc<SlashEntry>>,
}

fn entry(
    command: &Arc<AppCommand>,
    app: &AppInfo,
    path: Vec<String>,
    api_path: Vec<String>,
    args: &[CommandOption],
    description: &str,
) -> Arc<SlashEntry> {
    Arc::new(SlashEntry {
        command: command.clone(),
        display: path.join(" "),
        path,
        api_path,
        args: args.to_vec(),
        description: description.to_string(),
        app_name: app.name.clone(),
        app_id: command.application_id.clone(),
        app_icon_url: app.icon_url.clone(),
    })
}

/// Lo que se sabe de la app dueña de un comando.
struct AppInfo {
    name: String,
    icon_url: Option<String>,
}

/// Aplana el índice: solo `CHAT_INPUT` (los comandos de usuario/mensaje van en
/// menús contextuales, no con `/`).
pub fn build_catalog(index: CommandIndex) -> SlashCatalog {
    let CommandIndex { applications, commands } = index;
    let apps: HashMap<&str, &IndexApplication> = applications.iter().map(|app| (app.id.as_str(), app)).collect();
    let mut entries: Vec<Arc<SlashEntry>> = Vec::new();
    for command in commands {
        if command.kind != CHAT_INPUT || command.name.is_empty() {
            continue;
        }
        let app = AppInfo {
            name: apps.get(command.application_id.as_str()).map(|a| a.name.clone()).unwrap_or_default(),
            icon_url: apps.get(command.application_id.as_str()).and_then(|a| a.icon_url.clone()),
        };
        let command = Arc::new(command);
        let has_subcommands =
            command.options.iter().any(|o| o.kind == opt::SUB_COMMAND || o.kind == opt::SUB_COMMAND_GROUP);
        if !has_subcommands {
            entries.push(entry(
                &command,
                &app,
                vec![command.name.clone()],
                vec![command.api_name().to_string()],
                &command.options,
                &command.description,
            ));
            continue;
        }
        for option in &command.options {
            match option.kind {
                opt::SUB_COMMAND => entries.push(entry(
                    &command,
                    &app,
                    vec![command.name.clone(), option.name.clone()],
                    vec![command.api_name().to_string(), option.api_name().to_string()],
                    &option.options,
                    &option.description,
                )),
                opt::SUB_COMMAND_GROUP => {
                    for sub in option.options.iter().filter(|o| o.kind == opt::SUB_COMMAND) {
                        entries.push(entry(
                            &command,
                            &app,
                            vec![command.name.clone(), option.name.clone(), sub.name.clone()],
                            vec![
                                command.api_name().to_string(),
                                option.api_name().to_string(),
                                sub.api_name().to_string(),
                            ],
                            &sub.options,
                            &sub.description,
                        ));
                    }
                }
                _ => {}
            }
        }
    }
    entries.sort_by(|a, b| a.display.cmp(&b.display).then_with(|| a.app_name.cmp(&b.app_name)));
    SlashCatalog { entries }
}

/// Un catálogo guardado en `App` con su vencimiento (los comandos cambian
/// poco, pero un bot nuevo no debería tardar horas en aparecer).
#[derive(Clone, Debug)]
pub struct CachedCatalog {
    pub catalog: Arc<SlashCatalog>,
    pub fetched: Instant,
    pub ttl: Duration,
}

impl CachedCatalog {
    pub fn new(catalog: Arc<SlashCatalog>) -> Self {
        Self { catalog, fetched: Instant::now(), ttl: Duration::from_secs(600) }
    }

    /// Catálogo vacío por un pedido que falló: se reintenta en un rato corto
    /// en vez de dejar al usuario sin comandos por diez minutos.
    pub fn failed() -> Self {
        Self { catalog: Arc::new(SlashCatalog::default()), fetched: Instant::now(), ttl: Duration::from_secs(30) }
    }

    pub fn expired(&self) -> bool {
        self.fetched.elapsed() >= self.ttl
    }
}

// ---------------------------------------------------------------------
// Opciones: de lo tipeado a lo que pide la API
// ---------------------------------------------------------------------

/// `<@123>`, `<@!123>`, `<@&123>`, `<#123>` o `123` -> `123`.
pub fn extract_id(raw: &str) -> Option<String> {
    let id: String = raw.trim().trim_matches(|c: char| matches!(c, '<' | '>' | '@' | '!' | '&' | '#')).to_string();
    (!id.is_empty() && id.chars().all(|c| c.is_ascii_digit())).then_some(id)
}

/// Convierte lo escrito en una opción. `Ok(None)` = opcional y vacía (no se
/// manda); `Err` = mensaje para mostrarle a la persona.
pub fn option_value(option: &CommandOption, raw: &str) -> Result<Option<Value>, String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return if option.required { Err(format!("Falta completar «{}».", option.name)) } else { Ok(None) };
    }
    let value = match option.kind {
        opt::STRING => {
            let len = raw.chars().count() as u32;
            if let Some(min) = option.min_length.filter(|min| len < *min) {
                return Err(format!("«{}» necesita al menos {min} caracteres.", option.name));
            }
            if let Some(max) = option.max_length.filter(|max| len > *max) {
                return Err(format!("«{}» admite hasta {max} caracteres.", option.name));
            }
            Value::String(raw.to_string())
        }
        opt::INTEGER => {
            let n: i64 = trimmed
                .parse()
                .map_err(|_| format!("«{}» tiene que ser un número entero.", option.name))?;
            check_range(option, n as f64)?;
            json!(n)
        }
        opt::NUMBER => {
            let n: f64 = trimmed.parse().map_err(|_| format!("«{}» tiene que ser un número.", option.name))?;
            check_range(option, n)?;
            serde_json::Number::from_f64(n)
                .map(Value::Number)
                .ok_or_else(|| format!("«{}» no es un número válido.", option.name))?
        }
        opt::BOOLEAN => match trimmed.to_lowercase().as_str() {
            "true" | "verdadero" | "si" | "sí" | "yes" => Value::Bool(true),
            "false" | "falso" | "no" => Value::Bool(false),
            _ => return Err(format!("«{}» tiene que ser verdadero o falso.", option.name)),
        },
        opt::USER | opt::CHANNEL | opt::ROLE | opt::MENTIONABLE => Value::String(extract_id(trimmed).ok_or_else(|| {
            format!("Elige uno de la lista en «{}» (o escribe su ID o una mención, ej. <@123…>).", option.name)
        })?),
        opt::ATTACHMENT => return Err("Los adjuntos de comandos todavía no están soportados.".to_string()),
        _ => Value::String(raw.to_string()),
    };
    Ok(Some(value))
}

fn check_range(option: &CommandOption, n: f64) -> Result<(), String> {
    if let Some(min) = option.min_value.filter(|min| n < *min) {
        return Err(format!("«{}» tiene que ser al menos {min}.", option.name));
    }
    if let Some(max) = option.max_value.filter(|max| n > *max) {
        return Err(format!("«{}» tiene que ser {max} como máximo.", option.name));
    }
    Ok(())
}

/// Arma el arreglo `data.options` de la interacción a partir de lo escrito
/// (`values[i]` es el texto de `entry.args[i]`), envolviéndolo en el grupo y
/// el subcomando cuando la entrada es una hoja.
pub fn build_options(entry: &SlashEntry, values: &[String]) -> Result<Vec<Value>, String> {
    let mut leaf: Vec<Value> = Vec::new();
    for (index, option) in entry.args.iter().enumerate() {
        let raw = values.get(index).map(String::as_str).unwrap_or("");
        if let Some(value) = option_value(option, raw)? {
            leaf.push(json!({ "type": option.kind, "name": option.api_name(), "value": value }));
        }
    }
    Ok(match entry.api_path.as_slice() {
        [_, group, sub] => vec![json!({
            "type": opt::SUB_COMMAND_GROUP,
            "name": group,
            "options": [{ "type": opt::SUB_COMMAND, "name": sub, "options": leaf }],
        })],
        [_, sub] => vec![json!({ "type": opt::SUB_COMMAND, "name": sub, "options": leaf })],
        _ => leaf,
    })
}

/// Cuerpo de `POST /interactions` para ejecutar un slash command (tipo 2).
pub fn interaction_body(
    command: &AppCommand,
    options: Vec<Value>,
    channel_id: &str,
    guild_id: Option<&str>,
    session_id: &str,
    nonce: &str,
) -> Result<Value, serde_json::Error> {
    let mut body = json!({
        "type": 2,
        "nonce": nonce,
        "channel_id": channel_id,
        "application_id": command.application_id,
        "session_id": session_id,
        "analytics_location": "slash_ui",
        "data": {
            "version": command.version,
            "id": command.id,
            "name": command.api_name(),
            "type": command.kind,
            "options": options,
            "application_command": serde_json::to_value(command.for_api())?,
            "attachments": [],
        },
    });
    if let Some(guild_id) = guild_id {
        body["guild_id"] = json!(guild_id);
    }
    Ok(body)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn index() -> CommandIndex {
        parse_index(&json!({
            "applications": [{ "id": "10", "name": "Mod Bot" }],
            "application_commands": [
                {
                    "id": "1", "application_id": "10", "version": "99", "type": 1,
                    "name": "ping", "description": "Pong", "options": [
                        { "type": 3, "name": "texto", "description": "x", "required": true },
                        { "type": 4, "name": "veces", "description": "y", "min_value": 1, "max_value": 5 }
                    ]
                },
                {
                    "id": "2", "application_id": "10", "version": "100", "type": 1,
                    "name": "permisos", "description": "Permisos", "options": [
                        { "type": 2, "name": "usuario", "description": "g", "options": [
                            { "type": 1, "name": "ver", "description": "s", "options": [
                                { "type": 6, "name": "quien", "description": "u", "required": true }
                            ] }
                        ] },
                        { "type": 1, "name": "ayuda", "description": "h" }
                    ]
                },
                { "id": "3", "application_id": "10", "version": "1", "type": 2, "name": "Menu usuario", "description": "" },
                { "id": 4 }
            ]
        }))
    }

    #[test]
    fn catalog_flattens_subcommands_and_skips_context_menus() {
        let catalog = build_catalog(index());
        let names: Vec<&str> = catalog.entries.iter().map(|e| e.display.as_str()).collect();
        assert_eq!(names, vec!["permisos ayuda", "permisos usuario ver", "ping"]);
        assert!(catalog.entries.iter().all(|e| e.app_name == "Mod Bot"));
    }

    #[test]
    fn unreadable_commands_do_not_break_the_index() {
        // `{ "id": 4 }` tiene un id numérico: se descarta, el resto sigue.
        assert_eq!(index().commands.len(), 3);
    }

    #[test]
    fn options_are_nested_under_group_and_subcommand() {
        let catalog = build_catalog(index());
        let leaf = catalog.entries.iter().find(|e| e.display == "permisos usuario ver").unwrap();
        let options = build_options(leaf, &["<@123>".to_string()]).unwrap();
        assert_eq!(
            options,
            vec![json!({
                "type": 2, "name": "usuario",
                "options": [{ "type": 1, "name": "ver",
                    "options": [{ "type": 6, "name": "quien", "value": "123" }] }]
            })]
        );
    }

    #[test]
    fn plain_command_options_are_typed_and_validated() {
        let catalog = build_catalog(index());
        let ping = catalog.entries.iter().find(|e| e.display == "ping").unwrap();
        let ok = build_options(ping, &["hola".to_string(), "3".to_string()]).unwrap();
        assert_eq!(
            ok,
            vec![
                json!({ "type": 3, "name": "texto", "value": "hola" }),
                json!({ "type": 4, "name": "veces", "value": 3 }),
            ]
        );
        // Obligatoria vacía, entero inválido y fuera de rango.
        assert!(build_options(ping, &["".to_string(), "".to_string()]).is_err());
        assert!(build_options(ping, &["hola".to_string(), "abc".to_string()]).is_err());
        assert!(build_options(ping, &["hola".to_string(), "9".to_string()]).is_err());
        // La opcional vacía simplemente no se manda.
        let only_required = build_options(ping, &["hola".to_string(), "".to_string()]).unwrap();
        assert_eq!(only_required.len(), 1);
    }

    #[test]
    fn interaction_body_uses_original_name_and_scope() {
        let mut command = build_catalog(index()).entries[2].command.as_ref().clone();
        command.name = "pingue".to_string();
        command.name_default = Some("ping".to_string());
        let body = interaction_body(&command, vec![], "5", Some("7"), "sess", "123").unwrap();
        assert_eq!(body["type"], 2);
        assert_eq!(body["guild_id"], "7");
        assert_eq!(body["application_id"], "10");
        assert_eq!(body["data"]["name"], "ping");
        assert_eq!(body["data"]["application_command"]["name"], "ping");
        assert_eq!(body["data"]["version"], "99");
        let dm = interaction_body(&command, vec![], "5", None, "sess", "123").unwrap();
        assert!(dm.get("guild_id").is_none());
    }

    #[test]
    fn localized_option_and_subcommand_names_are_sent_in_original() {
        let index = parse_index(&json!({
            "applications": [{ "id": "10", "name": "Bot" }],
            "application_commands": [{
                "id": "1", "application_id": "10", "version": "9", "type": 1,
                "name": "perfil", "name_default": "profile", "description": "d",
                "options": [{
                    "type": 1, "name": "ver", "name_default": "view", "description": "s",
                    "options": [{ "type": 6, "name": "usuario", "name_default": "user",
                                  "description": "u", "required": true }]
                }]
            }]
        }));
        let catalog = build_catalog(index);
        let leaf = &catalog.entries[0];
        assert_eq!(leaf.display, "perfil ver");
        let options = build_options(leaf, &["<@5>".to_string()]).unwrap();
        assert_eq!(
            options,
            vec![json!({ "type": 1, "name": "view",
                "options": [{ "type": 6, "name": "user", "value": "5" }] })]
        );
        let body = interaction_body(&leaf.command, options, "5", Some("7"), "s", "1").unwrap();
        let sent = &body["data"]["application_command"]["options"][0];
        assert_eq!(sent["name"], "view");
        assert_eq!(sent["options"][0]["name"], "user");
    }

    #[test]
    fn extract_id_accepts_mentions_and_raw_ids() {
        assert_eq!(extract_id("<@!42>").as_deref(), Some("42"));
        assert_eq!(extract_id("<#42>").as_deref(), Some("42"));
        assert_eq!(extract_id("<@&42>").as_deref(), Some("42"));
        assert_eq!(extract_id(" 42 ").as_deref(), Some("42"));
        assert_eq!(extract_id("juan"), None);
    }
}
