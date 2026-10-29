use base64::Engine;
use discord_client_structs::structs::channel::invite::Invite;
use serde_json::json;

pub trait ContextHeader {
    fn get_header_value(&self) -> String;
}

#[derive(Debug, Clone)]
pub struct NoContext;

#[derive(Debug, Clone)]
pub struct NewGroupDmContext;

#[derive(Debug, Clone)]
pub struct InviteContext {
    pub location_guild_id: u64,
    pub location_channel_id: u64,
    pub location_channel_type: u8,
}

impl ContextHeader for NoContext {
    fn get_header_value(&self) -> String {
        json!({}).to_string()
    }
}

impl ContextHeader for NewGroupDmContext {
    fn get_header_value(&self) -> String {
        json!({"location":"New Group DM"}).to_string()
    }
}

impl ContextHeader for InviteContext {
    fn get_header_value(&self) -> String {
        json!({
            "location": "Join Guild",
            "location_guild_id": self.location_guild_id.to_string(),
            "location_channel_id": self.location_channel_id.to_string(),
            "location_channel_type": self.location_channel_type,
        })
        .to_string()
    }
}

/// Contexto simple de una sola clave: `{"location": "<lugar>"}`. Es la forma
/// de casi todos los `X-Context-Properties` del cliente oficial (el chat, las
/// pantallas de amigos, el menú contextual...). Las ubicaciones conocidas
/// están como constantes de [`Context`] (`Context::CHAT_INPUT`, ...).
#[derive(Debug, Clone)]
pub struct LocationContext {
    pub location: &'static str,
}

impl ContextHeader for LocationContext {
    fn get_header_value(&self) -> String {
        json!({ "location": self.location }).to_string()
    }
}

#[derive(Debug, Clone)]
pub enum Context {
    NoContext,
    NewGroupDmContext,
    InviteContext(InviteContext),
    Location(&'static str),
}

impl Context {
    /// Mandar un mensaje desde la caja de texto del chat.
    pub const CHAT_INPUT: Context = Context::Location("chat_input");
    /// Acciones hechas desde la pestaña de amigos (aceptar, quitar...).
    pub const FRIENDS: Context = Context::Location("Friends");
    /// Pantalla "Añadir amigo" (solicitud por nombre de usuario).
    pub const ADD_FRIEND: Context = Context::Location("Add Friend");
    /// Acciones hechas desde el perfil de una persona.
    pub const USER_PROFILE: Context = Context::Location("User Profile");
    /// Acciones del menú de clic derecho sobre una persona (así, sin espacio,
    /// lo escribe el cliente oficial en las acciones de relaciones).
    pub const CONTEXT_MENU: Context = Context::Location("ContextMenu");
}

impl ContextHeader for Context {
    fn get_header_value(&self) -> String {
        let ctx = match self {
            Context::NoContext => NoContext.get_header_value(),
            Context::NewGroupDmContext => NewGroupDmContext.get_header_value(),
            Context::InviteContext(ctx) => ctx.get_header_value(),
            Context::Location(location) => LocationContext { location }.get_header_value(),
        };

        base64::engine::general_purpose::STANDARD.encode(ctx.as_bytes())
    }
}

impl From<Invite> for Context {
    fn from(invite: Invite) -> Self {
        match invite.guild {
            Some(guild) => {
                let channel = invite.channel.unwrap();
                let id = channel.id;
                let r#type = channel.r#type;
                Context::InviteContext(InviteContext {
                    location_guild_id: guild.id,
                    location_channel_id: id,
                    location_channel_type: r#type,
                })
            }
            None => Context::NoContext,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Valores base64 que manda el cliente oficial (los mismos que usa
    // discord.py-self en `tracking.ContextProperties.LOCATIONS`).
    #[test]
    fn location_contexts_encode_like_the_official_client() {
        assert_eq!(Context::CHAT_INPUT.get_header_value(), "eyJsb2NhdGlvbiI6ImNoYXRfaW5wdXQifQ==");
        assert_eq!(Context::FRIENDS.get_header_value(), "eyJsb2NhdGlvbiI6IkZyaWVuZHMifQ==");
        assert_eq!(Context::ADD_FRIEND.get_header_value(), "eyJsb2NhdGlvbiI6IkFkZCBGcmllbmQifQ==");
        assert_eq!(Context::USER_PROFILE.get_header_value(), "eyJsb2NhdGlvbiI6IlVzZXIgUHJvZmlsZSJ9");
        assert_eq!(Context::CONTEXT_MENU.get_header_value(), "eyJsb2NhdGlvbiI6IkNvbnRleHRNZW51In0=");
        assert_eq!(Context::NoContext.get_header_value(), "e30=");
    }
}
