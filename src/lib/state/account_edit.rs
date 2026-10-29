//! Lo que `App` hace con las páginas de Ajustes que EDITAN la cuenta
//! (`ui::settings::account_settings`): mandar los cambios a Discord, aplicar el
//! resultado a `App::me`, guardar los interruptores en `settings-proto` y
//! manejar la lista de dispositivos.
//!
//! Los cambios de usuario salen por `discord::account_edit` (REST) y los de
//! configuración por `spawn_patch_user_settings` (proto). En los dos casos el
//! valor se aplica en `App` ANTES de que conteste Discord para que el
//! interruptor responda al instante; si Discord rechaza un cambio de usuario se
//! avisa y no se toca nada (esos solo se aplican cuando Discord los acepta).

use serde_json::{json, Value};

use super::{App, ToastKind};
use crate::discord::user_settings::PreloadedUserSettings;
use crate::discord::{AccountEdit, AccountEditOutcome, AccountEditTarget, AuthSession, User};
use crate::ui::settings::account_settings::{ProfileImage, Toggle};

/// Tamaño máximo de un avatar o banner (el límite de Discord).
const MAX_IMAGE_BYTES: u64 = 10 * 1024 * 1024;

/// Estado de las páginas de cuenta que vive en `App` (lo que se está
/// escribiendo vive en la memoria temporal de egui, ver `account_settings`).
#[derive(Default)]
pub struct AccountUi {
    /// Cambios de cuenta mandados que Discord todavía no contestó.
    pending: usize,
    /// El selector de archivos está abierto.
    picking: bool,
    /// Dispositivos con sesión abierta (`None` = todavía no se pidieron).
    pub sessions: Option<Vec<AuthSession>>,
    pub sessions_loading: bool,
    pub sessions_error: Option<String>,
}

impl AccountUi {
    /// ¿Hay un cambio en vuelo? Mientras tanto los botones de guardar se
    /// apagan para no mandar dos veces lo mismo.
    pub fn busy(&self) -> bool {
        self.pending > 0 || self.picking
    }
}

impl App {
    /// Manda uno o varios cambios de cuenta (nombre, usuario, correo,
    /// contraseña, avatar, perfil). Cada uno avisa su resultado por separado.
    pub fn submit_account_edits(&mut self, edits: Vec<AccountEdit>) {
        if edits.is_empty() {
            return;
        }
        let (Some(token), Some(tx)) = (self.discord_token.clone(), self.event_tx.clone()) else {
            self.push_toast(
                ToastKind::Warning,
                "Sin conexión",
                "Inicia sesión para cambiar los datos de tu cuenta.",
            );
            return;
        };
        self.account_ui.pending += edits.len();
        crate::discord::account_edit::spawn_account_edits(token, edits, tx);
    }

    /// Enciende o apaga un interruptor de la cuenta (`settings-proto/1`).
    pub fn set_account_toggle(&mut self, toggle: Toggle, value: bool) {
        let Some(token) = self.discord_token.clone() else {
            return;
        };
        let Some(settings) = self.discord_settings.as_mut() else {
            self.push_toast(
                ToastKind::Warning,
                "Ajustes sin cargar",
                "Todavía no llegaron los ajustes de tu cuenta; prueba en unos segundos.",
            );
            return;
        };
        // Discord reemplaza la sección de primer nivel ENTERA en cada `PATCH`
        // (los subcampos que no vengan vuelven a su valor por defecto), así que
        // se aplica el cambio acá y se manda la sección completa ya con él.
        // Todos los campos son `Option<bool>`: `Some(false)` también viaja.
        toggle.set(settings, value);
        let mut partial = PreloadedUserSettings::default();
        toggle.copy_section(settings, &mut partial);
        crate::discord::spawn_patch_user_settings(token, partial);
    }

    /// "Quién puede enviarte solicitudes de amistad" (`friend_source_flags`).
    pub fn set_friend_source(&mut self, flags: u32) {
        let Some(token) = self.discord_token.clone() else {
            return;
        };
        let Some(settings) = self.discord_settings.as_mut() else {
            return;
        };
        settings
            .privacy
            .get_or_insert_with(Default::default)
            .friend_source_flags = Some(flags);
        // Sección `privacy` entera (ver `set_account_toggle`).
        let partial = PreloadedUserSettings {
            privacy: settings.privacy.clone(),
            ..Default::default()
        };
        crate::discord::spawn_patch_user_settings(token, partial);
    }

    /// Estado de presencia de la propia cuenta tal cual lo guarda Discord en
    /// `settings-proto` (`status.status`): "online", "idle", "dnd" o
    /// "invisible". Sin ajustes cargados (o sin valor) cuenta como "online".
    pub fn own_status_key(&self) -> &str {
        self.discord_settings
            .as_ref()
            .and_then(|s| s.status.as_ref())
            .and_then(|s| s.status.as_deref())
            .filter(|s| !s.is_empty())
            .unwrap_or("online")
    }

    /// Lo mismo, como el `Status` que dibuja la UI ("invisible" se ve como
    /// desconectado: es lo que ven los demás).
    pub fn own_status(&self) -> crate::lib::data::Status {
        use crate::lib::data::Status;
        match self.own_status_key() {
            "idle" => Status::Idle,
            "dnd" => Status::Dnd,
            "invisible" | "offline" => Status::Offline,
            _ => Status::Online,
        }
    }

    /// Cambia tu estado de presencia ("online", "idle", "dnd", "invisible").
    /// Se aplica en `App` antes de que conteste Discord, para que el punto de
    /// estado cambie al instante.
    pub fn set_own_status(&mut self, key: &str) {
        let Some(token) = self.discord_token.clone() else {
            return;
        };
        let Some(settings) = self.discord_settings.as_mut() else {
            self.push_toast(
                ToastKind::Warning,
                "Ajustes sin cargar",
                "Todavía no llegaron los ajustes de tu cuenta; prueba en unos segundos.",
            );
            return;
        };
        settings.status.get_or_insert_with(Default::default).status = Some(key.to_owned());
        // Sección `status` entera (ver `set_account_toggle`): así no se pierde
        // el estado personalizado ni "compartir actividad".
        let partial = PreloadedUserSettings {
            status: settings.status.clone(),
            ..Default::default()
        };
        crate::discord::spawn_patch_user_settings(token, partial);
    }

    /// Abre el selector de archivos para el avatar o el banner. Al elegir una
    /// imagen válida se manda sola (`AppEvent::AccountEditPicked`).
    pub fn pick_profile_image(&mut self, which: ProfileImage) {
        if self.account_ui.picking {
            return;
        }
        let Some(tx) = self.event_tx.clone() else {
            return;
        };
        self.account_ui.picking = true;
        std::thread::spawn(move || {
            let picked = rfd::FileDialog::new()
                .set_title(which.dialog_title())
                .add_filter("Imágenes", &["png", "jpg", "jpeg", "gif", "webp"])
                .pick_file();
            let result = match picked {
                // Cerró el selector: nada que avisar.
                None => Err(String::new()),
                Some(path) => image_data_uri(&path).map(|uri| which.edit_with(json!(uri))),
            };
            let _ = tx.send(crate::discord::AppEvent::AccountEditPicked(result));
        });
    }

    /// Vuelve a pedir los dispositivos con sesión abierta.
    pub fn refresh_sessions(&mut self) {
        if self.account_ui.sessions_loading {
            return;
        }
        let (Some(token), Some(tx)) = (self.discord_token.clone(), self.event_tx.clone()) else {
            return;
        };
        self.account_ui.sessions_loading = true;
        self.account_ui.sessions_error = None;
        crate::discord::account_edit::spawn_fetch_sessions(token, tx);
    }

    /// Cierra la sesión de un dispositivo (`id_hash`).
    pub fn logout_session(&mut self, id_hash: String) {
        let (Some(token), Some(tx)) = (self.discord_token.clone(), self.event_tx.clone()) else {
            return;
        };
        self.account_ui.sessions_loading = true;
        self.account_ui.sessions_error = None;
        crate::discord::account_edit::spawn_logout_session(token, id_hash, tx);
    }

    // --- Resultados que vuelven de Discord -------------------------------

    pub(super) fn on_account_edit_picked(&mut self, result: Result<AccountEdit, String>) {
        self.account_ui.picking = false;
        match result {
            Ok(edit) => self.submit_account_edits(vec![edit]),
            // Selector cerrado sin elegir.
            Err(message) if message.is_empty() => {}
            Err(message) => self.push_toast(ToastKind::Warning, "No se pudo usar la imagen", message),
        }
    }

    pub(super) fn on_account_edit_result(&mut self, done: &'static str, result: Result<AccountEditOutcome, String>) {
        self.account_ui.pending = self.account_ui.pending.saturating_sub(1);
        let outcome = match result {
            Ok(outcome) => outcome,
            Err(message) => {
                self.push_toast(ToastKind::Warning, "No se pudo guardar", message);
                return;
            }
        };
        if let Some(me) = self.me.as_mut() {
            // Primero lo que se mandó (respaldo) y encima lo que contestó
            // Discord, que manda.
            merge_user(me, &outcome.sent);
            let response = match outcome.target {
                AccountEditTarget::Me => &outcome.response,
                // `PATCH /users/@me/profile` a veces anida el perfil.
                AccountEditTarget::Profile => outcome.response.get("user_profile").unwrap_or(&outcome.response),
            };
            merge_user(me, response);
        }
        // El selector de cuentas (nombre y avatar) también se actualiza.
        if let (Some(token), Some(me)) = (self.discord_token.clone(), self.me.as_ref()) {
            self.accounts.attach_profile(&token, me);
            self.accounts.save();
        }
        if let Some(new_token) = outcome.new_token {
            self.adopt_new_token(new_token);
        }
        self.push_toast(ToastKind::Success, done, "Guardado en tu cuenta de Discord.");
    }

    /// Cambiar la contraseña invalida el token anterior: se guarda el nuevo y se
    /// reconecta con él (si no, el próximo pedido daría 401 y cerraría sesión).
    fn adopt_new_token(&mut self, token: String) {
        self.accounts.upsert_token(&token);
        self.accounts.save();
        self.drop_connection();
        self.discord_token = Some(token.clone());
        self.reconnect_with_token(token);
    }

    pub(super) fn on_auth_sessions(&mut self, result: Result<Vec<AuthSession>, String>) {
        self.account_ui.sessions_loading = false;
        match result {
            Ok(list) => {
                self.account_ui.sessions = Some(list);
                self.account_ui.sessions_error = None;
            }
            Err(message) => self.account_ui.sessions_error = Some(message),
        }
    }

    pub(super) fn on_auth_session_closed(&mut self, id_hash: String, result: Result<(), String>) {
        match result {
            Ok(()) => {
                if let Some(list) = self.account_ui.sessions.as_mut() {
                    list.retain(|s| s.id_hash != id_hash);
                }
                self.account_ui.sessions_loading = false;
                self.push_toast(ToastKind::Success, "Sesión cerrada", "Ese dispositivo ya no tiene acceso a tu cuenta.");
                // Lo que Discord dice que queda, por si algo no coincide.
                self.refresh_sessions();
            }
            Err(message) => {
                self.account_ui.sessions_loading = false;
                self.account_ui.sessions_error = Some(message);
            }
        }
    }

    /// Un cambio de ajustes hecho en otro dispositivo (o el eco del nuestro):
    /// Discord manda las secciones que cambiaron COMPLETAS (`privacy`,
    /// `notifications`, `text_and_images`, `status`), así que cada una que venga
    /// reemplaza a la guardada (incluye `friend_source_flags`, que vive en
    /// `privacy`).
    pub(super) fn merge_account_toggles(current: &mut PreloadedUserSettings, incoming: &PreloadedUserSettings) {
        for toggle in Toggle::ALL {
            if toggle.has_section(incoming) {
                toggle.copy_section(incoming, current);
            }
        }
    }
}

/// Aplica a `me` los campos de usuario que traiga `value` (un `null` explícito
/// borra el campo; un campo ausente no lo toca).
fn merge_user(me: &mut User, value: &Value) {
    let Some(map) = value.as_object() else {
        return;
    };
    let text = |v: &Value| v.as_str().map(str::to_owned);
    for (key, v) in map {
        match key.as_str() {
            "username" => {
                if let Some(name) = text(v) {
                    me.username = name;
                }
            }
            "global_name" => me.global_name = text(v),
            "avatar" => me.avatar = text(v),
            "banner" => me.banner = text(v),
            "bio" => me.bio = text(v),
            "pronouns" => me.pronouns = text(v),
            "email" => me.email = text(v),
            "phone" => me.phone = text(v),
            "mfa_enabled" => me.mfa_enabled = v.as_bool(),
            "accent_color" => me.accent_color = v.as_u64().map(|c| c as u32),
            "banner_color" => me.banner_color = text(v),
            _ => {}
        }
    }
}

/// Lee una imagen y la devuelve como `data:image/...;base64,...`. El tipo sale
/// de los primeros bytes (no de la extensión) y se rechaza lo que Discord no
/// acepta o supera el límite.
fn image_data_uri(path: &std::path::Path) -> Result<String, String> {
    use base64::Engine;
    let meta = std::fs::metadata(path).map_err(|e| format!("No se pudo leer el archivo: {e}"))?;
    if meta.len() > MAX_IMAGE_BYTES {
        return Err("La imagen pesa más de 10 MB.".to_owned());
    }
    let bytes = std::fs::read(path).map_err(|e| format!("No se pudo leer el archivo: {e}"))?;
    let mime = if bytes.starts_with(&[0x89, b'P', b'N', b'G']) {
        "image/png"
    } else if bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
        "image/jpeg"
    } else if bytes.starts_with(b"GIF8") {
        "image/gif"
    } else if bytes.len() > 12 && &bytes[0..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        "image/webp"
    } else {
        return Err("Elige una imagen PNG, JPG, GIF o WEBP.".to_owned());
    };
    let encoded = base64::engine::general_purpose::STANDARD.encode(&bytes);
    Ok(format!("data:{mime};base64,{encoded}"))
}
