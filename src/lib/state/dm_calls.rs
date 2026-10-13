//! Llamadas de DM/grupo: quién está dentro, el ring entrante y su tarjeta.
//!
//! Vive como hijo de `lib::state` a propósito (igual que `memory`): así puede
//! leer los campos privados de `App` sin abrirlos al resto del crate.
//!
//! El flujo es:
//!
//! 1. El gateway manda `CALL_CREATE` / `CALL_UPDATE` (con `ringing`, los ids de
//!    quienes están sonando) y `CALL_DELETE`; `App::handle_discord_event` los
//!    anota acá en `App::dm_calls`. Los participantes salen del cache de voz
//!    (`VoiceScope::Private`), que ya mantiene `VOICE_STATE_UPDATE`.
//! 2. [`App::update_incoming_ring`] decide, en cada frame, si hay que estar
//!    sonando: el ring nombra a nuestra cuenta, todavía no estamos en esa
//!    llamada, no se rechazó y la cuenta NO está en "No molestar". Al empezar
//!    suena el tono (`support::ringtone`) y aparece la tarjeta
//!    (`ui::shell::incoming_call`); al dejar de cumplirse se corta todo.

use std::time::{Duration, Instant};

use super::{App, NotificationTarget};
use crate::discord::VoiceScope;
use crate::discord::voice::{parse_channel_id, parse_user_id};
use crate::lib::data::{VoiceOccupant, color_from_id};

/// Pasado este tiempo sin que nadie atienda ni Discord corte el ring, el tono
/// se apaga solo (la tarjeta también).
const RING_TIMEOUT: Duration = Duration::from_secs(90);

/// Lo que sabemos de una llamada de DM/grupo en curso.
#[derive(Clone, Debug, Default)]
pub struct DmCallInfo {
    /// Ids de usuario de quienes están sonando ahora mismo.
    pub ringing: Vec<String>,
}

/// La llamada entrante que está sonando (tarjeta + tono).
#[derive(Clone, Debug)]
pub struct IncomingRing {
    pub channel_id: String,
    pub title: String,
    pub body: String,
    pub avatar_url: Option<String>,
    pub initial: String,
    pub started: Instant,
}

impl App {
    /// `CALL_CREATE` / `CALL_UPDATE`.
    pub(super) fn apply_call_event(&mut self, channel_id: String, ringing: Vec<String>) {
        self.dm_calls.insert(channel_id, DmCallInfo { ringing });
        self.update_incoming_ring();
    }

    /// `CALL_DELETE`: la llamada terminó. Se olvidan los participantes ajenos
    /// que hubieran quedado en el cache (el estado propio lo maneja el
    /// `VOICE_STATE_UPDATE` que Discord manda al desconectarnos).
    pub(super) fn apply_call_deleted(&mut self, channel_id: &str) {
        self.dm_calls.remove(channel_id);
        self.declined_rings.remove(channel_id);
        if let Some(channel) = parse_channel_id(channel_id) {
            let me = self.me.as_ref().and_then(|me| parse_user_id(&me.id));
            self.clear_private_call_except(channel, me);
        }
        self.update_incoming_ring();
    }

    /// `true` con el estado de la cuenta en "No molestar" (ajustes de la
    /// cuenta). En ese estado no suenan las llamadas entrantes.
    pub fn is_dnd(&self) -> bool {
        self.discord_settings
            .as_ref()
            .and_then(|settings| settings.status.as_ref())
            .and_then(|status| status.status.as_deref())
            == Some("dnd")
    }

    /// Quién está dentro de la llamada del DM/grupo `channel_id`, ya con
    /// nombre y avatar, y quién está hablando (mismo orden que los tiles).
    pub fn dm_call_members(&self, channel_id: &str) -> (Vec<VoiceOccupant>, Vec<bool>) {
        let Some(id) = parse_channel_id(channel_id) else {
            return (Vec::new(), Vec::new());
        };
        let dm = self.dms.iter().find(|dm| dm.id == channel_id);
        let mut members = Vec::new();
        let mut speaking = Vec::new();
        for participant in self.private_call_participants(id) {
            let user_id = participant.user_id.to_string();
            let user = self
                .me
                .as_ref()
                .filter(|me| me.id == user_id)
                .or_else(|| dm.and_then(|dm| dm.recipients.iter().find(|user| user.id == user_id)));
            let (name, avatar_url, known) = match user {
                Some(user) => (user.display_name().to_string(), user.avatar_url(), true),
                None if !participant.display_name.is_empty() && participant.display_name != "Usuario desconocido" => {
                    (participant.display_name.clone(), None, true)
                }
                None => ("Usuario desconocido".to_string(), None, false),
            };
            members.push(VoiceOccupant {
                avatar_color: color_from_id(&user_id),
                user_id,
                name,
                avatar_url,
                known,
                self_mute: participant.self_mute || participant.mute,
                self_deaf: participant.self_deaf || participant.deaf,
                streaming: false,
            });
            speaking.push(participant.speaking);
        }
        (members, speaking)
    }

    /// ¿Hay alguien dentro de la llamada de ese DM/grupo? Es lo que muestra el
    /// ícono de teléfono + "En llamada" en la lista de mensajes directos.
    pub fn dm_call_active(&self, channel_id: &str) -> bool {
        parse_channel_id(channel_id).is_some_and(|id| self.private_call_has_participants(id))
    }

    /// ¿Hay que dibujar la vista de la llamada dentro del chat de ese DM/grupo?
    /// Sí cuando hay alguien dentro, o cuando acabamos de pedir unirnos y
    /// Discord todavía no confirmó (así se ve "Conectando…" en vez de nada).
    pub fn dm_call_visible(&self, channel_id: &str) -> bool {
        let Some(id) = parse_channel_id(channel_id) else {
            return false;
        };
        self.dm_call_active(channel_id)
            || self
                .voice_target
                .as_ref()
                .is_some_and(|target| target.scope == VoiceScope::Private(id))
    }

    /// Nombre del DM/grupo para mostrar en la barra de llamada y en la tarjeta.
    pub fn dm_call_label(&self, channel_id: &str) -> Option<String> {
        self.dms
            .iter()
            .find(|dm| dm.id == channel_id)
            .map(|dm| dm.username.clone())
            .filter(|name| !name.is_empty())
            .or_else(|| {
                self.friends
                    .iter()
                    .find(|friend| friend.dm_channel_id.as_deref() == Some(channel_id))
                    .map(|friend| friend.name.clone())
            })
    }

    /// Decide si tiene que estar sonando una llamada entrante y arranca o
    /// corta el tono y la tarjeta en consecuencia. Se llama cada frame (es
    /// barato) y al llegar un evento de llamada, así también reacciona a
    /// cambios que no son de llamada: pasar a "No molestar", entrar a la
    /// llamada desde otro dispositivo, etc.
    pub fn update_incoming_ring(&mut self) {
        let my_id = self.me.as_ref().map(|me| me.id.clone());

        // Un ring que ya no nombra a esta cuenta deja de contar como rechazado:
        // si vuelven a llamar, tiene que sonar de nuevo.
        if let Some(my_id) = my_id.as_deref() {
            let dm_calls = &self.dm_calls;
            self.declined_rings.retain(|channel_id| {
                dm_calls
                    .get(channel_id)
                    .is_some_and(|call| call.ringing.iter().any(|id| id == my_id))
            });
        }
        // Tope de seguridad: un ring que nadie corta no suena para siempre.
        if let Some(ring) = &self.incoming_ring
            && ring.started.elapsed() > RING_TIMEOUT
        {
            self.declined_rings.insert(ring.channel_id.clone());
        }

        let wanted: Option<String> = match my_id.as_deref() {
            Some(my_id) if !self.is_dnd() => self
                .dm_calls
                .iter()
                .filter(|(channel_id, call)| {
                    call.ringing.iter().any(|id| id == my_id)
                        && !self.declined_rings.contains(*channel_id)
                        && !self.is_connected_to_voice_dm_str(channel_id)
                })
                .map(|(channel_id, _)| channel_id.clone())
                .min(),
            _ => None,
        };

        if self.incoming_ring.as_ref().map(|ring| ring.channel_id.as_str()) == wanted.as_deref() {
            return;
        }

        // Cambió: se corta lo anterior (soltar el handle apaga el tono).
        self.ring_sound = None;
        self.incoming_ring = None;
        let Some(channel_id) = wanted else {
            return;
        };

        let dm = self.dms.iter().find(|dm| dm.id == channel_id);
        let (title, body, avatar_url, initial) = match dm {
            Some(dm) => {
                let body = if dm.is_group() {
                    "Llamada de grupo entrante"
                } else {
                    "Te está llamando"
                };
                (
                    dm.username.clone(),
                    body.to_string(),
                    dm.avatar_url.clone().or_else(|| dm.group_icon_url()),
                    super::author_initial(&dm.username),
                )
            }
            None => (
                "Llamada entrante".to_string(),
                "Te están llamando".to_string(),
                None,
                "?".to_string(),
            ),
        };

        if !self.window_focused {
            crate::lib::notifications::send_desktop(title.clone(), body.clone());
        }
        self.ring_sound = Some(crate::support::ringtone::start());
        self.incoming_ring = Some(IncomingRing {
            channel_id,
            title,
            body,
            avatar_url,
            initial,
            started: Instant::now(),
        });
    }

    /// Botón verde de la tarjeta: abre el DM y se une a la llamada.
    pub fn accept_incoming_call(&mut self) {
        let Some(ring) = self.incoming_ring.take() else {
            return;
        };
        self.ring_sound = None;
        // Hasta que Discord confirme que ya estamos dentro, el ring sigue
        // nombrándonos: se marca como atendido para que no vuelva a sonar.
        self.declined_rings.insert(ring.channel_id.clone());
        self.open_notification_target(&NotificationTarget::Dm {
            channel_id: ring.channel_id.clone(),
        });
        self.start_dm_call(&ring.channel_id);
    }

    /// Botón rojo de la tarjeta: corta el tono y avisa a Discord que no
    /// vamos a atender.
    pub fn decline_incoming_call(&mut self) {
        let Some(ring) = self.incoming_ring.take() else {
            return;
        };
        self.ring_sound = None;
        self.declined_rings.insert(ring.channel_id.clone());
        let me = self.me.as_ref().map(|me| me.id.clone());
        if let (Some(token), Some(me)) = (self.discord_token.clone(), me) {
            crate::discord::spawn_stop_ringing(token, ring.channel_id, vec![me]);
        }
    }
}
