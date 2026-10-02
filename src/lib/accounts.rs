//! Cuentas de Discord guardadas en este equipo (multi-cuenta).
//!
//! Antes ecord guardaba UN solo token (`current_user` en el almacenamiento
//! local). Ahora se guarda una lista de cuentas en `accounts.json`, dentro del
//! directorio de estado del usuario (ver `paths::accounts_file`), con permisos
//! privados (`0600` en Unix).
//!
//! Una cuenta con `token == None` sigue en la lista pero está "con la sesión
//! cerrada" (cerrada a mano o rechazada por Discord): el selector de cuentas la
//! muestra para que la persona sepa que tiene que volver a iniciar sesión.
//!
//! Los tokens se guardan igual que antes, en texto plano: quien lea ese archivo
//! puede entrar a las cuentas. Por eso el archivo es privado y por eso
//! "Cerrar sesión" / "Quitar cuenta" también invalidan el token en Discord.

use base64::Engine as _;
use serde::{Deserialize, Serialize};

use crate::discord::models::User;

const FILE_VERSION: u32 = 1;

/// Una cuenta recordada. El perfil (nombre/avatar) es lo último que se vio en
/// el `READY`, para poder dibujar el selector sin conectarse.
#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct SavedAccount {
    /// Id de Discord. Puede estar vacío un instante, entre guardar el token de
    /// un login nuevo y recibir el `READY` (ver `AccountStore::attach_profile`).
    #[serde(default)]
    pub user_id: String,
    #[serde(default)]
    pub username: String,
    #[serde(default)]
    pub display_name: String,
    #[serde(default)]
    pub avatar_url: Option<String>,
    /// `None` = sesión cerrada o expirada: hay que volver a iniciar sesión.
    #[serde(default)]
    pub token: Option<String>,
}

impl SavedAccount {
    pub fn needs_login(&self) -> bool {
        self.token.is_none()
    }

    /// Nombre para mostrar (con respaldos para una cuenta recién agregada).
    pub fn label(&self) -> &str {
        if !self.display_name.is_empty() {
            &self.display_name
        } else if !self.username.is_empty() {
            &self.username
        } else {
            "Cuenta nueva"
        }
    }
}

#[derive(Serialize, Deserialize, Debug, Default)]
pub struct AccountStore {
    #[serde(default)]
    version: u32,
    #[serde(default)]
    accounts: Vec<SavedAccount>,
    /// Última cuenta con la que se estuvo conectado: es la que se abre sola al
    /// arrancar ecord.
    #[serde(default)]
    last_used: Option<String>,
}

/// Formato viejo (una sola cuenta), solo para migrarlo.
#[derive(Deserialize, Default)]
struct LegacyUserDb {
    #[serde(default)]
    logged_in: bool,
    #[serde(default)]
    token: Option<String>,
}

/// El primer tramo de un token de usuario de Discord es el id de la cuenta en
/// base64. Sirve para identificar la cuenta antes de que llegue el `READY`.
fn user_id_from_token(token: &str) -> Option<String> {
    let first = token.split('.').next()?.trim_end_matches('=');
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(first)
        .or_else(|_| base64::engine::general_purpose::STANDARD_NO_PAD.decode(first))
        .ok()?;
    let id = String::from_utf8(bytes).ok()?;
    (!id.is_empty() && id.len() <= 20 && id.bytes().all(|b| b.is_ascii_digit())).then_some(id)
}

impl AccountStore {
    /// Lee `accounts.json` y, si todavía hay una sesión del formato viejo, la
    /// migra (y la borra del almacenamiento local para no dejar el token
    /// duplicado).
    pub fn load() -> Self {
        let mut store = Self::read_file().unwrap_or_default();

        if let Ok(Some(json)) = web_local_storage_api::get_item("current_user") {
            let legacy = serde_json::from_str::<LegacyUserDb>(&json).unwrap_or_default();
            if let Some(token) = legacy.token.filter(|t| legacy.logged_in && !t.is_empty()) {
                store.upsert_token(&token);
                if store.save() {
                    let _ = web_local_storage_api::set_item(
                        "current_user",
                        r#"{"logged_in":false,"token":null}"#,
                    );
                }
            }
        }
        store
    }

    fn read_file() -> Option<Self> {
        let path = crate::paths::accounts_file()?;
        let raw = std::fs::read_to_string(path).ok()?;
        serde_json::from_str(&raw).ok()
    }

    /// Guarda en disco (privado). `false` si no se pudo.
    pub fn save(&self) -> bool {
        let Some(path) = crate::paths::accounts_file() else {
            return false;
        };
        if let Some(dir) = path.parent() {
            if std::fs::create_dir_all(dir).is_err() {
                return false;
            }
            let _ = crate::support::private_file::set_private_dir_permissions(dir);
        }
        let mut to_write = Self {
            version: FILE_VERSION,
            accounts: self.accounts.clone(),
            last_used: self.last_used.clone(),
        };
        // Una cuenta sin id ni token no sirve para nada.
        to_write.accounts.retain(|a| !(a.user_id.is_empty() && a.token.is_none()));
        match serde_json::to_string_pretty(&to_write) {
            Ok(json) => crate::support::private_file::write_private_file(&path, json).is_ok(),
            Err(_) => false,
        }
    }

    pub fn accounts(&self) -> &[SavedAccount] {
        &self.accounts
    }

    pub fn is_empty(&self) -> bool {
        self.accounts.is_empty()
    }

    pub fn get(&self, user_id: &str) -> Option<&SavedAccount> {
        self.accounts.iter().find(|a| a.user_id == user_id)
    }

    /// Token de la cuenta que se abre sola al arrancar. Si no hay "última
    /// usada" (formato viejo) y hay una sola cuenta con sesión, esa. `None` →
    /// se muestra el selector de cuentas.
    pub fn startup_token(&self) -> Option<String> {
        if let Some(id) = &self.last_used {
            return self.get(id).and_then(|a| a.token.clone());
        }
        let mut with_token = self.accounts.iter().filter_map(|a| a.token.clone());
        let first = with_token.next()?;
        with_token.next().is_none().then_some(first)
    }

    /// Guarda el token de un login recién hecho (QR o contraseña). Si ya había
    /// una entrada para esa cuenta (misma sesión o mismo id), se actualiza.
    pub fn upsert_token(&mut self, token: &str) {
        let decoded = user_id_from_token(token);
        let index = self
            .accounts
            .iter()
            .position(|a| a.token.as_deref() == Some(token))
            .or_else(|| {
                decoded
                    .as_ref()
                    .and_then(|id| self.accounts.iter().position(|a| &a.user_id == id))
            });
        match index {
            Some(i) => {
                self.accounts[i].token = Some(token.to_owned());
                if self.accounts[i].user_id.is_empty() {
                    if let Some(id) = decoded.clone() {
                        self.accounts[i].user_id = id;
                    }
                }
            }
            None => self.accounts.push(SavedAccount {
                user_id: decoded.clone().unwrap_or_default(),
                token: Some(token.to_owned()),
                ..SavedAccount::default()
            }),
        }
        if let Some(id) = decoded {
            self.last_used = Some(id);
        }
    }

    /// Completa nombre/avatar/id de la cuenta que acaba de conectarse (viene
    /// del `READY`) y la marca como la última usada.
    pub fn attach_profile(&mut self, token: &str, user: &User) {
        let by_token = self.accounts.iter().position(|a| a.token.as_deref() == Some(token));
        let by_id = self.accounts.iter().position(|a| a.user_id == user.id);
        let index = match (by_token, by_id) {
            // La entrada sin id de un login nuevo y otra ya conocida de la
            // misma cuenta: se queda la conocida, con el token nuevo.
            (Some(t), Some(i)) if t != i => {
                self.accounts[i].token = Some(token.to_owned());
                self.accounts.remove(t);
                if t < i { i - 1 } else { i }
            }
            (Some(t), _) => t,
            (None, Some(i)) => {
                self.accounts[i].token = Some(token.to_owned());
                i
            }
            (None, None) => {
                self.accounts.push(SavedAccount {
                    token: Some(token.to_owned()),
                    ..SavedAccount::default()
                });
                self.accounts.len() - 1
            }
        };
        let account = &mut self.accounts[index];
        account.user_id = user.id.clone();
        account.username = user.username.clone();
        account.display_name = user.display_name().to_owned();
        account.avatar_url = user.avatar_url();
        self.last_used = Some(user.id.clone());
    }

    /// Deja la cuenta con la sesión cerrada: sigue en la lista, sin token.
    pub fn sign_out_token(&mut self, token: &str) {
        if let Some(account) = self.accounts.iter_mut().find(|a| a.token.as_deref() == Some(token)) {
            account.token = None;
        }
        self.accounts.retain(|a| !(a.user_id.is_empty() && a.token.is_none()));
    }

    /// Saca la cuenta de la lista. Devuelve su token (si tenía) para poder
    /// invalidarlo en Discord.
    pub fn remove(&mut self, user_id: &str) -> Option<String> {
        let index = self.accounts.iter().position(|a| a.user_id == user_id)?;
        let removed = self.accounts.remove(index);
        if self.last_used.as_deref() == Some(user_id) {
            self.last_used = None;
        }
        removed.token
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // base64("123456789012345678") = "MTIzNDU2Nzg5MDEyMzQ1Njc4"
    const TOKEN_A: &str = "MTIzNDU2Nzg5MDEyMzQ1Njc4.abcdef.secret";

    #[test]
    fn decodes_user_id_from_token() {
        assert_eq!(user_id_from_token(TOKEN_A).as_deref(), Some("123456789012345678"));
        assert_eq!(user_id_from_token("no-es-un-token"), None);
    }

    #[test]
    fn upsert_twice_keeps_one_account_and_sign_out_clears_token() {
        let mut store = AccountStore::default();
        store.upsert_token(TOKEN_A);
        store.upsert_token(TOKEN_A);
        assert_eq!(store.accounts().len(), 1);
        assert_eq!(store.startup_token().as_deref(), Some(TOKEN_A));

        store.sign_out_token(TOKEN_A);
        assert_eq!(store.accounts().len(), 1);
        assert!(store.accounts()[0].needs_login());
        assert_eq!(store.startup_token(), None);
    }

    #[test]
    fn remove_returns_token() {
        let mut store = AccountStore::default();
        store.upsert_token(TOKEN_A);
        assert_eq!(store.remove("123456789012345678").as_deref(), Some(TOKEN_A));
        assert!(store.is_empty());
    }
}
