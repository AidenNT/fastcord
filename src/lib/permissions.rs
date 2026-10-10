//! Cálculo LOCAL de qué canales de un server puede ver / usar la cuenta,
//! con el algoritmo de permisos de Discord (roles base + overwrites del
//! canal). Discord manda TODOS los canales del server —también los que tu
//! rol no puede ver— y es el cliente el que los filtra, así que acá hacemos
//! lo mismo.
//!
//! Todo es "fail-open": si falta cualquier dato necesario (permisos de los
//! roles, roles propios, el rol @everyone) el resultado es "visible y sin
//! candado". Nunca escondemos un canal por no tener información.
//!
//! Orden de aplicación (docs de Discord, "Permission Overwrites"):
//! 1. Dueño del server o ADMINISTRATOR → todos los permisos.
//! 2. Base = permisos de @everyone + los de cada rol propio.
//! 3. Overwrite de @everyone del canal.
//! 4. Overwrites de los roles propios (se juntan los deny y los allow).
//! 5. Overwrite propio (miembro).

use crate::discord::models::{PermissionOverwrite, Role};

pub const ADMINISTRATOR: u64 = 1 << 3;
pub const VIEW_CHANNEL: u64 = 1 << 10;
pub const MENTION_EVERYONE: u64 = 1 << 17;
pub const MANAGE_MESSAGES: u64 = 1 << 13;
pub const CONNECT: u64 = 1 << 20;
pub const ADD_REACTIONS: u64 = 1 << 6;
pub const EMBED_LINKS: u64 = 1 << 14;
pub const ATTACH_FILES: u64 = 1 << 15;
pub const SEND_MESSAGES: u64 = 1 << 11;
pub const READ_MESSAGE_HISTORY: u64 = 1 << 16;
/// Desde el 23/02/2026 es el ÚNICO permiso que exime del modo lento (antes
/// también lo hacían Administrar mensajes / canales / hilos).
pub const BYPASS_SLOWMODE: u64 = 1 << 52;

/// Qué tan accesible es un canal para la cuenta. El `Default` es "visible,
/// sin candado" (el caso fail-open).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ChannelAccess {
    /// La cuenta puede ver el canal. Si es `false`, la UI lo esconde.
    pub can_view: bool,
    /// El canal es privado: @everyone NO lo ve, o sea que la cuenta lo ve
    /// gracias a un rol u overwrite propio. Se marca con candado.
    pub private: bool,
    /// Solo canales de voz: la cuenta puede verlo pero no conectarse.
    /// Se marca con candado.
    pub cannot_connect: bool,
    /// Permiso "Enviar mensajes". Sin él el compositor se bloquea.
    pub can_send: bool,
    /// Permiso "Adjuntar archivos".
    pub can_attach: bool,
    /// Permiso "Insertar enlaces" (sin él los links y GIFs no se expanden).
    pub can_embed: bool,
    /// Permiso "Ver el historial de mensajes": sin él no hay historial, solo
    /// se ven los mensajes que llegan mientras se está mirando el canal.
    pub can_read_history: bool,
    /// Permiso "Añadir reacciones" (reacciones NUEVAS: sumarse a una que ya
    /// existe sí se puede).
    pub can_react: bool,
    /// Exento del modo lento del canal (permiso "Saltarse el modo lento",
    /// Administrador o dueño). Fail-open: si no se sabe, se asume exento para
    /// no bloquear el envío por error.
    pub bypass_slowmode: bool,
}

impl Default for ChannelAccess {
    fn default() -> Self {
        Self {
            can_view: true,
            private: false,
            cannot_connect: false,
            can_send: true,
            can_attach: true,
            can_embed: true,
            can_read_history: true,
            can_react: true,
            bypass_slowmode: true,
        }
    }
}

impl ChannelAccess {
    /// ¿Hay que dibujar el candado?
    pub fn show_lock(&self) -> bool {
        self.private || self.cannot_connect
    }
}

/// Lo que hace falta saber del server y de la cuenta para calcular
/// permisos. Vive en `Server` y se va completando a medida que llegan los
/// datos (roles, roles propios, dueño).
#[derive(Clone, Debug, Default)]
pub struct AccessContext {
    pub owner_id: Option<String>,
    /// Id de la cuenta logueada.
    pub my_id: String,
    /// Roles propios en este server. `None` = todavía no se sabe.
    pub my_roles: Option<Vec<String>>,
}

/// Permisos base (sin overwrites) de un miembro con `member_roles`.
/// `None` si falta el rol @everyone o los permisos de algún rol.
fn base_permissions(guild_id: &str, roles: &[Role], member_roles: &[String]) -> Option<u64> {
    let everyone = roles.iter().find(|r| r.id == guild_id)?;
    let mut perms = everyone.permissions?;
    for id in member_roles {
        // Un rol que ya no existe se ignora (Discord hace lo mismo).
        if let Some(role) = roles.iter().find(|r| &r.id == id) {
            perms |= role.permissions?;
        }
    }
    Some(perms)
}

/// Aplica los overwrites de un canal sobre los permisos base.
fn apply_overwrites(
    mut perms: u64,
    guild_id: &str,
    overwrites: &[PermissionOverwrite],
    member_roles: &[String],
    member_id: Option<&str>,
) -> u64 {
    if perms & ADMINISTRATOR != 0 {
        return u64::MAX;
    }
    if let Some(ow) = overwrites.iter().find(|o| o.kind == 0 && o.id == guild_id) {
        perms &= !ow.deny;
        perms |= ow.allow;
    }
    let (mut allow, mut deny) = (0u64, 0u64);
    for ow in overwrites
        .iter()
        .filter(|o| o.kind == 0 && member_roles.contains(&o.id))
    {
        allow |= ow.allow;
        deny |= ow.deny;
    }
    perms &= !deny;
    perms |= allow;
    if let Some(id) = member_id {
        if let Some(ow) = overwrites.iter().find(|o| o.kind == 1 && o.id == id) {
            perms &= !ow.deny;
            perms |= ow.allow;
        }
    }
    perms
}

/// Calcula el acceso de la cuenta a un canal. `is_voice` decide si además
/// se mira el permiso de conectarse.
pub fn channel_access(
    guild_id: &str,
    ctx: &AccessContext,
    roles: &[Role],
    overwrites: &[PermissionOverwrite],
    is_voice: bool,
) -> ChannelAccess {
    let open = ChannelAccess::default();
    // Sin roles propios no se puede saber nada: fail-open.
    let Some(my_roles) = ctx.my_roles.as_deref() else {
        return open;
    };
    if ctx.my_id.is_empty() {
        return open;
    }
    if ctx.owner_id.as_deref() == Some(ctx.my_id.as_str()) {
        return open;
    }
    let Some(base) = base_permissions(guild_id, roles, my_roles) else {
        return open;
    };

    let mine = if base & ADMINISTRATOR != 0 {
        u64::MAX
    } else {
        apply_overwrites(base, guild_id, overwrites, my_roles, Some(ctx.my_id.as_str()))
    };
    let can_view = mine & VIEW_CHANNEL != 0;
    if !can_view {
        return ChannelAccess {
            can_view: false,
            private: false,
            cannot_connect: false,
            ..ChannelAccess::default()
        };
    }

    // "Privado" = un miembro sin roles ni overwrite propio tampoco lo ve.
    // (`base` de @everyone solo, más el overwrite de @everyone del canal.)
    let everyone_base = roles
        .iter()
        .find(|r| r.id == guild_id)
        .and_then(|r| r.permissions)
        .unwrap_or(0);
    let everyone_perms = apply_overwrites(everyone_base, guild_id, overwrites, &[], None);
    let private = everyone_perms & VIEW_CHANNEL == 0;

    let cannot_connect = is_voice && mine & CONNECT == 0;

    ChannelAccess {
        can_view: true,
        private,
        cannot_connect,
        can_send: mine & SEND_MESSAGES != 0,
        can_attach: mine & ATTACH_FILES != 0,
        can_embed: mine & EMBED_LINKS != 0,
        can_read_history: mine & READ_MESSAGE_HISTORY != 0,
        can_react: mine & ADD_REACTIONS != 0,
        bypass_slowmode: mine & BYPASS_SLOWMODE != 0,
    }
}

/// ¿La cuenta puede mencionar a `@everyone`, `@here` y a cualquier rol (aunque
/// no sea "mencionable")? Mira solo los permisos base del server (sin los
/// overwrites del canal). Fail-open como el resto del módulo: si todavía no se
/// sabe (roles propios o permisos sin cargar), se asume que sí — mostrar de más
/// en el menú de menciones es mejor que esconderlo.
pub fn can_mention_everyone(guild_id: &str, ctx: &AccessContext, roles: &[Role]) -> bool {
    let Some(my_roles) = ctx.my_roles.as_deref() else {
        return true;
    };
    if ctx.my_id.is_empty() || ctx.owner_id.as_deref() == Some(ctx.my_id.as_str()) {
        return true;
    }
    let Some(base) = base_permissions(guild_id, roles, my_roles) else {
        return true;
    };
    base & (ADMINISTRATOR | MENTION_EVERYONE) != 0
}

/// ¿La cuenta puede borrar mensajes ajenos y sacar reacciones de otros en un
/// canal (permiso "Administrar mensajes")? A diferencia del resto del módulo
/// es fail-closed: si no se sabe, se asume que NO (mejor no ofrecer borrar
/// que ofrecerlo y que falle).
pub fn can_manage_messages(
    guild_id: &str,
    ctx: &AccessContext,
    roles: &[Role],
    overwrites: &[PermissionOverwrite],
) -> bool {
    if ctx.my_id.is_empty() {
        return false;
    }
    if ctx.owner_id.as_deref() == Some(ctx.my_id.as_str()) {
        return true;
    }
    let Some(my_roles) = ctx.my_roles.as_deref() else {
        return false;
    };
    let Some(base) = base_permissions(guild_id, roles, my_roles) else {
        return false;
    };
    if base & ADMINISTRATOR != 0 {
        return true;
    }
    apply_overwrites(base, guild_id, overwrites, my_roles, Some(ctx.my_id.as_str())) & MANAGE_MESSAGES != 0
}

#[cfg(test)]
mod tests {
    use super::*;

    const GUILD: &str = "1";
    const ME: &str = "42";
    /// Lo que un @everyone normal puede hacer en el chat (sin saltarse el
    /// modo lento).
    const CHAT_PERMS: u64 =
        SEND_MESSAGES | ATTACH_FILES | EMBED_LINKS | READ_MESSAGE_HISTORY | ADD_REACTIONS;

    fn role(id: &str, perms: u64) -> Role {
        Role {
            id: id.to_owned(),
            permissions: Some(perms),
            ..Default::default()
        }
    }

    fn ow(id: &str, kind: u8, allow: u64, deny: u64) -> PermissionOverwrite {
        PermissionOverwrite {
            id: id.to_owned(),
            kind,
            allow,
            deny,
        }
    }

    fn ctx(my_roles: Option<Vec<&str>>) -> AccessContext {
        AccessContext {
            owner_id: None,
            my_id: ME.to_owned(),
            my_roles: my_roles.map(|r| r.into_iter().map(str::to_owned).collect()),
        }
    }

    fn roles() -> Vec<Role> {
        vec![
            role(GUILD, VIEW_CHANNEL | CONNECT | CHAT_PERMS),
            role("mod", 0),
            role("admin", ADMINISTRATOR),
        ]
    }

    #[test]
    fn public_channel_is_open() {
        let a = channel_access(GUILD, &ctx(Some(vec![])), &roles(), &[], false);
        // Abierto del todo, salvo que un @everyone común no se salta el modo lento.
        assert_eq!(
            a,
            ChannelAccess {
                bypass_slowmode: false,
                ..ChannelAccess::default()
            }
        );
    }

    #[test]
    fn hidden_channel_is_not_viewable() {
        let overwrites = [ow(GUILD, 0, 0, VIEW_CHANNEL)];
        let a = channel_access(GUILD, &ctx(Some(vec![])), &roles(), &overwrites, false);
        assert!(!a.can_view);
    }

    #[test]
    fn hidden_channel_visible_through_role_is_private() {
        let overwrites = [ow(GUILD, 0, 0, VIEW_CHANNEL), ow("mod", 0, VIEW_CHANNEL, 0)];
        let a = channel_access(GUILD, &ctx(Some(vec!["mod"])), &roles(), &overwrites, false);
        assert!(a.can_view && a.private && a.show_lock());
    }

    #[test]
    fn member_overwrite_beats_role_overwrite() {
        let overwrites = [
            ow(GUILD, 0, 0, VIEW_CHANNEL),
            ow("mod", 0, VIEW_CHANNEL, 0),
            ow(ME, 1, 0, VIEW_CHANNEL),
        ];
        let a = channel_access(GUILD, &ctx(Some(vec!["mod"])), &roles(), &overwrites, false);
        assert!(!a.can_view);
    }

    #[test]
    fn administrator_sees_everything_and_private_is_locked() {
        let overwrites = [ow(GUILD, 0, 0, VIEW_CHANNEL)];
        let a = channel_access(GUILD, &ctx(Some(vec!["admin"])), &roles(), &overwrites, true);
        assert!(a.can_view && a.private && !a.cannot_connect);
    }

    #[test]
    fn owner_is_never_filtered() {
        let mut c = ctx(Some(vec![]));
        c.owner_id = Some(ME.to_owned());
        let overwrites = [ow(GUILD, 0, 0, VIEW_CHANNEL)];
        assert_eq!(
            channel_access(GUILD, &c, &roles(), &overwrites, false),
            ChannelAccess::default()
        );
    }

    #[test]
    fn voice_without_connect_is_locked_but_visible() {
        let overwrites = [ow(GUILD, 0, 0, CONNECT)];
        let a = channel_access(GUILD, &ctx(Some(vec![])), &roles(), &overwrites, true);
        assert!(a.can_view && !a.private && a.cannot_connect && a.show_lock());
    }

    #[test]
    fn text_channel_ignores_connect() {
        let overwrites = [ow(GUILD, 0, 0, CONNECT)];
        let a = channel_access(GUILD, &ctx(Some(vec![])), &roles(), &overwrites, false);
        assert!(!a.show_lock());
    }

    #[test]
    fn mention_everyone_needs_the_permission() {
        let roles = [
            role(GUILD, VIEW_CHANNEL),
            role("mod", MENTION_EVERYONE),
            role("admin", ADMINISTRATOR),
        ];
        assert!(!can_mention_everyone(GUILD, &ctx(Some(vec![])), &roles));
        assert!(can_mention_everyone(GUILD, &ctx(Some(vec!["mod"])), &roles));
        assert!(can_mention_everyone(GUILD, &ctx(Some(vec!["admin"])), &roles));
        // Datos que faltan: fail-open.
        assert!(can_mention_everyone(GUILD, &ctx(None), &roles));
    }

    #[test]
    fn chat_permissions_follow_overwrites() {
        // @everyone: sin enviar, sin adjuntar, sin historial ni reacciones.
        let overwrites = [ow(
            GUILD,
            0,
            0,
            SEND_MESSAGES | ATTACH_FILES | READ_MESSAGE_HISTORY | ADD_REACTIONS,
        )];
        let a = channel_access(GUILD, &ctx(Some(vec![])), &roles(), &overwrites, false);
        assert!(a.can_view && !a.can_send && !a.can_attach && !a.can_read_history && !a.can_react);
        assert!(a.can_embed);
        // Un rol que lo vuelve a permitir.
        let overwrites = [
            ow(GUILD, 0, 0, SEND_MESSAGES),
            ow("mod", 0, SEND_MESSAGES, 0),
        ];
        let a = channel_access(GUILD, &ctx(Some(vec!["mod"])), &roles(), &overwrites, false);
        assert!(a.can_send);
    }

    #[test]
    fn slowmode_is_only_bypassed_with_the_permission() {
        let plain = channel_access(GUILD, &ctx(Some(vec![])), &roles(), &[], false);
        assert!(!plain.bypass_slowmode);
        let all_roles = [
            role(GUILD, VIEW_CHANNEL | CHAT_PERMS),
            role("free", BYPASS_SLOWMODE),
            role("admin", ADMINISTRATOR),
        ];
        let free = channel_access(GUILD, &ctx(Some(vec!["free"])), &all_roles, &[], false);
        assert!(free.bypass_slowmode);
        let admin = channel_access(GUILD, &ctx(Some(vec!["admin"])), &all_roles, &[], false);
        assert!(admin.bypass_slowmode && admin.can_send);
        // Sin datos: fail-open (exento).
        let unknown = channel_access(GUILD, &ctx(None), &all_roles, &[], false);
        assert!(unknown.bypass_slowmode && unknown.can_send);
    }

    #[test]
    fn unknown_data_fails_open() {
        let overwrites = [ow(GUILD, 0, 0, VIEW_CHANNEL)];
        // Roles propios desconocidos.
        assert_eq!(
            channel_access(GUILD, &ctx(None), &roles(), &overwrites, false),
            ChannelAccess::default()
        );
        // Sin @everyone / sin permisos en los roles.
        assert_eq!(
            channel_access(GUILD, &ctx(Some(vec![])), &[], &overwrites, false),
            ChannelAccess::default()
        );
        let unknown_perms = [Role {
            id: GUILD.to_owned(),
            permissions: None,
            ..Default::default()
        }];
        assert_eq!(
            channel_access(GUILD, &ctx(Some(vec![])), &unknown_perms, &overwrites, false),
            ChannelAccess::default()
        );
    }
}
