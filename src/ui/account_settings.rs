//! Pestaña "Cuenta" del panel de ajustes: muestra, SOLO EN LECTURA, los
//! ajustes que Discord guarda en la cuenta (`PreloadedUserSettings`, ver
//! `discord::user_settings`) más los datos básicos del usuario logueado.
//!
//! No hay ninguna `Action` acá a propósito: nada de esto se edita desde
//! `ecord` (para eso hace falta mandar un `PATCH` al `settings-proto`, que
//! por ahora solo usamos para el tema). La vista se arma en dos pasos para
//! no clonar el blob entero cada frame:
//!
//! 1. [`build`] recorre `App::me` / `App::discord_settings` y devuelve una
//!    lista chica de [`Section`] (título + filas etiqueta/valor ya
//!    formateadas como texto).
//! 2. [`view`] solo dibuja esa lista.
//!
//! Para los enums del proto usamos `as_str_name()` (el nombre original del
//! `.proto`, p. ej. `"THEME_DARK"`) en vez de hacer `match` contra las
//! variantes de Rust: así no dependemos de cómo `prost` renombra cada
//! variante y un valor nuevo que agregue Discord cae en un texto genérico
//! en vez de romper la compilación.

use egui::{CornerRadius, Frame, Margin, Vec2};

use crate::discord::models::User;
use crate::discord::user_settings::PreloadedUserSettings;
use crate::ui::theme::{self, Icon, Palette};

/// Lo que se muestra cuando un valor no está definido en la cuenta.
const UNSET: &str = "—";

/// Una fila "etiqueta: valor" ya formateada.
pub struct Row {
    label: &'static str,
    value: String,
}

/// Un grupo de filas bajo un título.
pub struct Section {
    title: &'static str,
    rows: Vec<Row>,
}

impl Section {
    fn new(title: &'static str) -> Self {
        Self {
            title,
            rows: Vec::new(),
        }
    }

    fn row(&mut self, label: &'static str, value: impl Into<String>) -> &mut Self {
        self.rows.push(Row {
            label,
            value: value.into(),
        });
        self
    }
}

// --- Formateo de valores ------------------------------------------------

fn yes_no(v: bool) -> String {
    let text = if v { "Sí" } else { "No" };
    text.to_owned()
}

fn opt_bool(v: Option<bool>) -> String {
    v.map(yes_no).unwrap_or_else(|| UNSET.to_owned())
}

fn opt_string(v: Option<&str>) -> String {
    match v {
        Some(s) if !s.is_empty() => s.to_owned(),
        _ => UNSET.to_owned(),
    }
}

fn opt_num<T: std::fmt::Display>(v: Option<T>) -> String {
    v.map(|n| n.to_string()).unwrap_or_else(|| UNSET.to_owned())
}

/// Milisegundos desde epoch → fecha legible (UTC). `0` es "nunca/no hay"
/// en todos los campos `*_ms` del proto.
fn fmt_ms(ms: u64) -> Option<String> {
    if ms == 0 {
        return None;
    }
    let dt = chrono::DateTime::from_timestamp_millis(i64::try_from(ms).ok()?)?;
    Some(dt.format("%Y-%m-%d %H:%M UTC").to_string())
}

/// Traduce el nombre crudo de un enum del proto a español; si es uno que
/// no conocemos (Discord agrega valores seguido) lo deja legible en vez de
/// esconderlo.
fn enum_label(raw: &str) -> String {
    let known = match raw {
        // Tema
        "THEME_UNSET" => "Sin definir",
        "THEME_DARK" => "Oscuro",
        "THEME_LIGHT" => "Claro",
        "THEME_DARKER" => "Más oscuro",
        "THEME_MIDNIGHT" => "Medianoche",
        // Formato de hora
        "TIMESTAMP_HOUR_CYCLE_AUTO" => "Automático",
        "TIMESTAMP_HOUR_CYCLE_H12" => "12 horas",
        "TIMESTAMP_HOUR_CYCLE_H23" => "24 horas",
        // Densidad de la interfaz
        "UI_DENSITY_UNSET_UI_DENSITY" => "Sin definir",
        "UI_DENSITY_COMPACT" => "Compacta",
        "UI_DENSITY_COZY" => "Cómoda",
        "UI_DENSITY_RESPONSIVE" => "Adaptable",
        "UI_DENSITY_DEFAULT" => "Predeterminada",
        // Filtro de spam en MD
        "DM_SPAM_FILTER_V2_DEFAULT_UNSET" => "Sin definir",
        "DM_SPAM_FILTER_V2_DISABLED" => "Desactivado",
        "DM_SPAM_FILTER_V2_NON_FRIENDS" => "Solo de quienes no son amigos",
        "DM_SPAM_FILTER_V2_FRIENDS_AND_NON_FRIENDS" => "De todos",
        // Contenido sensible (mostrar / difuminar / bloquear)
        "EXPLICIT_CONTENT_REDACTION_UNSET_EXPLICIT_CONTENT_REDACTION" => "Sin definir",
        "EXPLICIT_CONTENT_REDACTION_SHOW" => "Mostrar",
        "EXPLICIT_CONTENT_REDACTION_BLUR" => "Difuminar",
        "EXPLICIT_CONTENT_REDACTION_BLOCK" => "Bloquear",
        // Buscador
        "SEARCH_PROVIDER_UNSET" => "Sin definir",
        "SEARCH_PROVIDER_GOOGLE" => "Google",
        "SEARCH_PROVIDER_BING" => "Bing",
        "SEARCH_PROVIDER_DUCKDUCKGO" => "DuckDuckGo",
        "SEARCH_PROVIDER_CUSTOM" => "Personalizado",
        // Notificaciones
        "REACTION_NOTIFICATION_TYPE_NOTIFICATIONS_ENABLED" => "Activadas",
        "REACTION_NOTIFICATION_TYPE_ONLY_DMS" => "Solo en mensajes directos",
        "REACTION_NOTIFICATION_TYPE_NOTIFICATIONS_DISABLED" => "Desactivadas",
        "GAME_ACTIVITY_NOTIFICATION_TYPE_ACTIVITY_NOTIFICATIONS_UNSET" => "Sin definir",
        "GAME_ACTIVITY_NOTIFICATION_TYPE_ACTIVITY_NOTIFICATIONS_DISABLED" => "Desactivadas",
        "GAME_ACTIVITY_NOTIFICATION_TYPE_ACTIVITY_NOTIFICATIONS_ENABLED" => "Activadas",
        "GAME_ACTIVITY_NOTIFICATION_TYPE_ONLY_GAMES_PLAYED" => "Solo de juegos que jugás",
        "CUSTOM_STATUS_PUSH_NOTIFICATION_TYPE_STATUS_PUSH_UNSET" => "Sin definir",
        "CUSTOM_STATUS_PUSH_NOTIFICATION_TYPE_STATUS_PUSH_ENABLED" => "Activadas",
        "CUSTOM_STATUS_PUSH_NOTIFICATION_TYPE_STATUS_PUSH_DISABLED" => "Desactivadas",
        // Privacidad
        "PROFILE_VISIBILITY_UNSET" => "Sin definir",
        "PROFILE_VISIBILITY_FRIENDS_ONLY" => "Solo amigos",
        "PROFILE_VISIBILITY_FRIENDS_AND_SMALL_GUILDS" => "Amigos y servidores chicos",
        "PROFILE_VISIBILITY_FRIENDS_AND_ALL_GUILDS" => "Amigos y todos los servidores",
        "GUILD_ACTIVITY_STATUS_RESTRICTION_DEFAULT_V2_ACTIVITY_STATUS_UNSET" => "Sin definir",
        "GUILD_ACTIVITY_STATUS_RESTRICTION_DEFAULT_V2_ACTIVITY_STATUS_OFF" => "Desactivada",
        "GUILD_ACTIVITY_STATUS_RESTRICTION_DEFAULT_V2_ACTIVITY_STATUS_ON_FOR_LARGE_GUILDS" => {
            "Activada en servidores grandes"
        }
        "GUILD_ACTIVITY_STATUS_RESTRICTION_DEFAULT_V2_ACTIVITY_STATUS_ON" => "Activada",
        // Seguridad
        "SAFETY_SETTINGS_PRESET_TYPE_UNSET_SAFETY_SETTINGS_PRESET" => "Sin definir",
        "SAFETY_SETTINGS_PRESET_TYPE_BALANCED" => "Equilibrado",
        "SAFETY_SETTINGS_PRESET_TYPE_STRICT" => "Estricto",
        "SAFETY_SETTINGS_PRESET_TYPE_RELAXED" => "Relajado",
        "SAFETY_SETTINGS_PRESET_TYPE_CUSTOM" => "Personalizado",
        _ => "",
    };
    if !known.is_empty() {
        return known.to_owned();
    }
    // Desconocido: el nombre crudo en minúsculas y sin guiones bajos.
    raw.to_lowercase().replace('_', " ")
}

/// Estado de presencia tal cual lo guarda Discord (`"online"`, `"idle"`…).
fn presence_label(raw: &str) -> String {
    let text = match raw {
        "online" => "En línea",
        "idle" => "Ausente",
        "dnd" => "No molestar",
        "invisible" => "Invisible",
        "offline" => "Desconectado",
        other => other,
    };
    text.to_owned()
}

/// `render_spoilers`: `"ALWAYS"`, `"ON_CLICK"` o `"IF_MODERATOR"`.
fn spoilers_label(raw: &str) -> String {
    let text = match raw {
        "ALWAYS" => "Siempre visibles",
        "ON_CLICK" => "Al hacer clic",
        "IF_MODERATOR" => "Si sos moderador",
        other => other,
    };
    text.to_owned()
}

/// Nivel de Nitro del `READY` (mismo mapeo que documenta `User`).
fn nitro_label(user: &User) -> String {
    match user.premium_type {
        Some(0) => "Ninguno".to_owned(),
        Some(1) => "Nitro Classic".to_owned(),
        Some(2) => "Nitro".to_owned(),
        Some(3) => "Nitro Basic".to_owned(),
        Some(n) => format!("Desconocido ({n})"),
        None => opt_bool(user.has_nitro()),
    }
}

/// `animate_stickers`: 0 = siempre, 1 = al pasar el mouse, 2 = nunca.
fn animate_stickers_label(v: u32) -> String {
    match v {
        0 => "Siempre".to_owned(),
        1 => "Al pasar el mouse".to_owned(),
        2 => "Nunca".to_owned(),
        n => format!("Desconocido ({n})"),
    }
}

/// `explicit_content_filter` (campo viejo): 0/1/2.
fn legacy_explicit_filter_label(v: u32) -> String {
    match v {
        0 => "No analizar ningún mensaje".to_owned(),
        1 => "Analizar mensajes de miembros sin rol".to_owned(),
        2 => "Analizar todos los mensajes".to_owned(),
        n => format!("Desconocido ({n})"),
    }
}

/// `friend_source_flags`: bit 0 = amigos de amigos, bit 1 = miembros de
/// servers en común, bit 2 = todos.
fn friend_source_label(flags: u32) -> String {
    if flags & 0b100 != 0 {
        return "Todos".to_owned();
    }
    let mut parts = Vec::new();
    if flags & 0b001 != 0 {
        parts.push("Amigos de amigos");
    }
    if flags & 0b010 != 0 {
        parts.push("Miembros de servidores en común");
    }
    if parts.is_empty() {
        "Nadie".to_owned()
    } else {
        parts.join(", ")
    }
}

// --- Armado de las secciones ---------------------------------------------

/// Arma todas las secciones a mostrar. `settings` en `None` (todavía no
/// llegó el `settings-proto`, o el pedido falló) devuelve solo la sección
/// de la cuenta si hay usuario; [`view`] avisa aparte que faltan los
/// ajustes.
pub fn build(me: Option<&User>, settings: Option<&PreloadedUserSettings>) -> Vec<Section> {
    let mut out = Vec::new();

    if let Some(user) = me {
        let mut s = Section::new("Mi cuenta");
        s.row("Nombre para mostrar", user.display_name())
            .row("Nombre de usuario", opt_string(Some(user.username.as_str())))
            .row("ID de usuario", opt_string(Some(user.id.as_str())))
            .row("Nitro", nitro_label(user))
            .row("Pronombres", opt_string(user.pronouns.as_deref()));
        if let Some(bio) = user.bio.as_deref().filter(|b| !b.is_empty()) {
            s.row("Sobre mí", bio);
        }
        out.push(s);
    }

    let Some(cfg) = settings else {
        return out;
    };

    // Apariencia
    if let Some(a) = cfg.appearance.as_ref() {
        let mut s = Section::new("Apariencia");
        s.row("Tema", enum_label(a.theme().as_str_name()))
            .row("Densidad de la interfaz", enum_label(a.ui_density().as_str_name()))
            .row(
                "Formato de hora",
                enum_label(a.timestamp_hour_cycle().as_str_name()),
            )
            .row("Modo desarrollador", yes_no(a.developer_mode))
            .row("Barra lateral oscura", yes_no(a.dark_sidebar))
            .row(
                "Conteo exacto de resultados de búsqueda",
                opt_bool(a.search_result_exact_count_enabled),
            );
        if let Some(preset) = a
            .client_theme_settings
            .as_ref()
            .and_then(|c| c.background_gradient_preset_id)
        {
            s.row("Degradado de fondo (preset)", preset.to_string());
        }
        out.push(s);
    }

    // Idioma y región
    if let Some(l) = cfg.localization.as_ref() {
        let mut s = Section::new("Idioma y región");
        s.row("Idioma", opt_string(l.locale.as_deref()))
            .row("Zona horaria", opt_string(l.timezone_name.as_deref()))
            .row(
                "Diferencia horaria (minutos)",
                opt_num(l.timezone_offset),
            );
        out.push(s);
    }

    // Estado
    if let Some(st) = cfg.status.as_ref() {
        let mut s = Section::new("Estado");
        s.row(
            "Estado de presencia",
            st.status
                .as_deref()
                .filter(|v| !v.is_empty())
                .map(presence_label)
                .unwrap_or_else(|| UNSET.to_owned()),
        )
        .row("Mostrar juego actual", opt_bool(st.show_current_game));
        if let Some(cs) = st.custom_status.as_ref() {
            s.row("Estado personalizado", opt_string(Some(cs.text.as_str())));
            if !cs.emoji_name.is_empty() {
                s.row("Emoji del estado", cs.emoji_name.clone());
            }
            s.row(
                "El estado personalizado expira",
                fmt_ms(cs.expires_at_ms).unwrap_or_else(|| "Nunca".to_owned()),
            );
        }
        out.push(s);
    }

    // Texto e imágenes
    if let Some(t) = cfg.text_and_images.as_ref() {
        let mut s = Section::new("Texto e imágenes");
        s.row("Mensajes compactos", opt_bool(t.message_display_compact))
            .row("Reproducir GIFs automáticamente", opt_bool(t.gif_auto_play))
            .row("Animar emojis", opt_bool(t.animate_emoji))
            .row(
                "Animar stickers",
                t.animate_stickers
                    .map(animate_stickers_label)
                    .unwrap_or_else(|| UNSET.to_owned()),
            )
            .row("Mostrar embeds", opt_bool(t.render_embeds))
            .row("Mostrar reacciones", opt_bool(t.render_reactions))
            .row(
                "Mostrar adjuntos como imagen/video",
                opt_bool(t.inline_attachment_media),
            )
            .row(
                "Mostrar contenido de enlaces",
                opt_bool(t.inline_embed_media),
            )
            .row(
                "Spoilers",
                t.render_spoilers
                    .as_deref()
                    .filter(|v| !v.is_empty())
                    .map(spoilers_label)
                    .unwrap_or_else(|| UNSET.to_owned()),
            )
            .row("Convertir emoticones en emojis", opt_bool(t.convert_emoticons))
            .row("Comando /tts", opt_bool(t.enable_tts_command))
            .row(
                "Sugerencias de comandos",
                opt_bool(t.show_command_suggestions),
            )
            .row(
                "Ver descripciones de imágenes",
                opt_bool(t.view_image_descriptions),
            )
            .row("Ver servidores NSFW", opt_bool(t.view_nsfw_guilds))
            .row("Buscador", enum_label(t.search_provider().as_str_name()))
            .row(
                "Filtro de spam en mensajes directos",
                enum_label(t.dm_spam_filter_v2().as_str_name()),
            );
        if let Some(v) = t.explicit_content_filter {
            s.row("Filtro de contenido explícito", legacy_explicit_filter_label(v));
        }
        if let Some(e) = t.explicit_content_settings.as_ref() {
            s.row(
                "Contenido sensible en servidores",
                enum_label(e.explicit_content_guilds().as_str_name()),
            )
            .row(
                "Contenido sensible de amigos (MD)",
                enum_label(e.explicit_content_friend_dm().as_str_name()),
            )
            .row(
                "Contenido sensible de otros (MD)",
                enum_label(e.explicit_content_non_friend_dm().as_str_name()),
            );
        }
        out.push(s);
    }

    // Notificaciones
    if let Some(n) = cfg.notifications.as_ref() {
        let mut s = Section::new("Notificaciones");
        s.row(
            "Notificaciones dentro de la app",
            opt_bool(n.show_in_app_notifications),
        )
        .row("Modo silencioso", opt_bool(n.quiet_mode))
        .row(
            "Modo concentración hasta",
            fmt_ms(n.focus_mode_expires_at_ms).unwrap_or_else(|| "No activo".to_owned()),
        )
        .row(
            "Notificaciones de reacciones",
            enum_label(n.reaction_notifications().as_str_name()),
        )
        .row(
            "Notificaciones de reacciones súper",
            opt_bool(n.enable_burst_reaction_notifications),
        )
        .row(
            "Actividad de juegos",
            enum_label(n.game_activity_notifications().as_str_name()),
        )
        .row(
            "Estado personalizado de amigos (push)",
            enum_label(n.custom_status_push_notifications().as_str_name()),
        )
        .row(
            "Actividad de voz",
            opt_bool(n.enable_voice_activity_notifications),
        )
        .row(
            "Amigos que se conectan",
            opt_bool(n.enable_friend_online_notifications),
        )
        .row(
            "Aniversarios de amistad",
            opt_bool(n.enable_friend_anniversary_notifications),
        )
        .row(
            "Eventos de servidor próximos",
            opt_bool(n.enable_upcoming_server_event_notifications),
        )
        .row("Avisar a amigos cuando hago directo", opt_bool(n.notify_friends_on_go_live));
        out.push(s);
    }

    // Privacidad
    if let Some(p) = cfg.privacy.as_ref() {
        let mut s = Section::new("Privacidad");
        s.row(
            "Visibilidad del perfil",
            enum_label(p.profile_visibility().as_str_name()),
        )
        .row(
            "Quién puede agregarme como amigo",
            p.friend_source_flags
                .map(friend_source_label)
                .unwrap_or_else(|| UNSET.to_owned()),
        )
        .row(
            "Restringir mensajes directos en servidores nuevos",
            yes_no(p.default_guilds_restricted),
        )
        .row(
            "Restringir solicitudes de mensaje en servidores nuevos",
            opt_bool(p.default_message_request_restricted),
        )
        .row(
            "Servidores con mensajes directos restringidos",
            p.restricted_guild_ids.len().to_string(),
        )
        .row(
            "Estado de actividad en servidores nuevos",
            enum_label(p.default_guilds_activity_restricted_v2().as_str_name()),
        )
        .row(
            "Servidores con actividad oculta",
            p.activity_restricted_guild_ids.len().to_string(),
        )
        .row(
            "Invitaciones a actividad (amigos)",
            opt_bool(p.allow_activity_party_privacy_friends),
        )
        .row(
            "Invitaciones a actividad (canal de voz)",
            opt_bool(p.allow_activity_party_privacy_voice_channel),
        )
        .row("Mostrar hora local", opt_bool(p.show_local_time))
        .row("Ocultar nombre de usuario antiguo", opt_bool(p.hide_legacy_username))
        .row("Juegos recientes", opt_bool(p.recent_games_enabled))
        .row(
            "Avisos de conversaciones inapropiadas",
            opt_bool(p.inappropriate_conversation_warnings),
        )
        .row("Centro de familia", opt_bool(p.family_center_enabled_v2))
        .row("Sincronizar contactos", opt_bool(p.contact_sync_enabled))
        .row(
            "Detectar cuentas de otras plataformas",
            opt_bool(p.detect_platform_accounts),
        )
        .row(
            "Permitir detección de accesibilidad",
            yes_no(p.allow_accessibility_detection),
        );
        out.push(s);
    }

    // Voz y video (lo que guarda la cuenta; los dispositivos locales viven
    // en la pestaña "Voz y audio").
    if let Some(v) = cfg.voice_and_video.as_ref() {
        let mut s = Section::new("Voz y video (cuenta)");
        s.row("Vista previa de video siempre", opt_bool(v.always_preview_video))
            .row(
                "Tiempo de inactividad (AFK, segundos)",
                opt_num(v.afk_timeout),
            )
            .row(
                "Notificaciones de transmisiones",
                opt_bool(v.stream_notifications_enabled),
            )
            .row(
                "Desactivar vistas previas de transmisiones",
                opt_bool(v.disable_stream_previews),
            )
            .row(
                "Integración con teléfono",
                opt_bool(v.native_phone_integration_enabled),
            );
        if let Some(sb) = v.soundboard_settings.as_ref() {
            s.row("Volumen del tablero de sonidos", format!("{:.0}%", sb.volume));
        }
        out.push(s);
    }

    // Seguridad
    if let Some(sf) = cfg.safety_settings.as_ref() {
        let mut s = Section::new("Seguridad");
        s.row(
            "Preset de seguridad",
            enum_label(sf.safety_settings_preset().as_str_name()),
        );
        out.push(s);
    }

    // Servidores y organización
    {
        let mut s = Section::new("Servidores y organización");
        if let Some(f) = cfg.guild_folders.as_ref() {
            let folders = f.folders.iter().filter(|x| x.id.is_some()).count();
            let servers: usize = f.folders.iter().map(|x| x.guild_ids.len()).sum();
            s.row("Carpetas de servidores", folders.to_string())
                .row("Servidores ordenados", servers.to_string());
        }
        if let Some(g) = cfg.guilds.as_ref() {
            s.row("Servidores con ajustes propios", g.guilds.len().to_string());
        }
        if let Some(fav) = cfg.favorites.as_ref() {
            s.row("Canales favoritos", fav.favorite_channels.len().to_string());
        }
        if let Some(app) = cfg.applications.as_ref() {
            s.row("Aplicaciones con ajustes", app.app_settings.len().to_string());
        }
        if !s.rows.is_empty() {
            out.push(s);
        }
    }

    // Versiones (útil para diagnosticar sincronización)
    if let Some(v) = cfg.versions.as_ref() {
        let mut s = Section::new("Sincronización");
        s.row("Versión del cliente", v.client_version.to_string())
            .row("Versión del servidor", v.server_version.to_string())
            .row("Versión de los datos", v.data_version.to_string());
        out.push(s);
    }

    out
}

// --- Dibujo ---------------------------------------------------------------

/// Dibuja la pestaña. `settings_loaded` distingue "todavía no llegaron los
/// ajustes" de "llegaron pero esa sección está vacía".
pub fn view(ui: &mut egui::Ui, palette: &Palette, sections: &[Section], settings_loaded: bool) {
    // Aviso de solo lectura, arriba de todo.
    Frame::new()
        .fill(palette.surface)
        .corner_radius(CornerRadius::same(theme::radius()))
        .inner_margin(Margin::symmetric(12, 10))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                theme::icon(ui, Icon::Lock, 14.0, palette.dim);
                theme::text(
                    ui,
                    "Solo lectura: estos son los ajustes de tu cuenta tal como los guarda Discord.",
                    theme::regular(12.5),
                    palette.secondary,
                );
            });
        });
    ui.add_space(14.0);

    if !settings_loaded {
        theme::subtle(
            ui,
            palette,
            "Los ajustes de tu cuenta todavía no se cargaron (o Discord no los devolvió).",
        );
        ui.add_space(14.0);
    }

    for section in sections {
        theme::section_title(ui, palette, section.title);
        ui.add_space(8.0);
        Frame::new()
            .fill(palette.surface)
            .corner_radius(CornerRadius::same(theme::radius()))
            .inner_margin(Margin::symmetric(14, 8))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                for (i, row) in section.rows.iter().enumerate() {
                    if i > 0 {
                        ui.separator();
                    }
                    setting_row(ui, palette, row);
                }
            });
        ui.add_space(18.0);
    }
}

/// Una fila: la etiqueta en una columna fija a la izquierda y el valor a
/// continuación. El valor es seleccionable (se puede copiar) pero no
/// editable, y hace wrap si es largo (p. ej. "Sobre mí").
fn setting_row(ui: &mut egui::Ui, palette: &Palette, row: &Row) {
    ui.horizontal_top(|ui| {
        ui.add_sized(
            Vec2::new(260.0, 20.0),
            egui::Label::new(
                egui::RichText::new(row.label)
                    .font(theme::regular(13.0))
                    .color(palette.secondary),
            )
            .truncate(),
        );
        ui.add(
            egui::Label::new(
                egui::RichText::new(&row.value)
                    .font(theme::medium(13.0))
                    .color(palette.text),
            )
            .wrap_mode(egui::TextWrapMode::Wrap)
            .selectable(true),
        );
    });
}
