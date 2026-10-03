//! Informe de memoria de `App`: en qué se va la RAM y la caché.
//!
//! Vive como hijo de `lib::state` a propósito: así puede leer los campos
//! privados de `App` (presencias, actividades, menciones...) sin tener que
//! abrirlos al resto del crate.
//!
//! Los números de "Datos de Discord" son ESTIMACIONES (ver
//! `support::mem_report::HeapBytes`). "Funcionamiento" es lo que sobra de la
//! RAM real del proceso al restar todo lo medido; ahí caen el propio programa,
//! la UI, las fuentes, el audio/voz, el video (ffmpeg) y la fragmentación del
//! allocator.

use std::time::Duration;

use prost::Message as _;

use super::App;
use crate::support::mem_report::{self, DiskStats, HeapBytes};

/// Cuánto pesan en RAM los datos que bajó el cliente de Discord (bytes).
#[derive(Clone, Copy, Debug, Default)]
pub struct DiscordMemory {
    /// Mensajes de todos los canales, hilos y DMs que se llegaron a cargar.
    pub messages: usize,
    /// Servidores: canales, categorías, roles, emojis, stickers, permisos.
    pub structure: usize,
    /// Miembros: listas por servidor, apodos, roles, usuarios conocidos.
    pub members: usize,
    /// Lista de amigos/DMs (sin contar los mensajes, que van en `messages`).
    pub friends_dms: usize,
    /// Tu usuario, ajustes de la cuenta, presencias, actividades, menciones,
    /// notificaciones, estado de voz.
    pub account: usize,
}

impl DiscordMemory {
    pub fn total(&self) -> usize {
        self.messages + self.structure + self.members + self.friends_dms + self.account
    }
}

/// Foto completa de la memoria para la pestaña Ajustes → Memoria.
#[derive(Clone, Copy, Debug, Default)]
pub struct MemoryReport {
    /// RAM real del proceso según el sistema (`None` si no la informa).
    pub resident: Option<usize>,
    /// Heap de Rust vivo (exacto).
    pub heap: usize,
    /// `resident`, o `heap` si el sistema no informa la RAM del proceso.
    pub total: usize,
    /// Funcionamiento: `total` menos todo lo medido abajo.
    pub base: usize,
    pub discord: DiscordMemory,
    /// Imágenes ya decodificadas que guarda `egui`.
    pub images_decoded: usize,
    /// Archivos bajados que se guardan en RAM (`support::http_cache`).
    pub images_downloaded: usize,
    /// Cuadros de GIF/APNG animados (son texturas: viven en la GPU).
    pub animations: usize,
    /// Todas las texturas de `egui` (GPU).
    pub gpu_total: usize,
    /// Reproductores de video / GIF en mp4 (`egui_video::Engine`).
    pub video: egui_video::EngineMem,
    /// Texturas de las miniaturas del selector de GIFs (GPU).
    pub gif_thumbs: usize,
    /// De qué está hecha la RAM que no es heap de Rust (solo Windows).
    pub native: mem_report::NativeBreakdown,
    pub disk: DiskStats,
}

impl App {
    /// Estima cuánto ocupan en RAM los datos de Discord que tiene `App`.
    pub fn discord_memory(&self) -> DiscordMemory {
        let mut mem = DiscordMemory::default();

        // Servidores. `Server::heap_bytes` incluye todo (también los
        // mensajes y los miembros); se reparte restando esas dos partes.
        mem.structure += self.servers.capacity() * size_of::<crate::lib::data::Server>();
        for server in &self.servers {
            let messages: usize = server
                .categories
                .iter()
                .flat_map(|category| category.channels.iter())
                .map(|channel| channel.messages.heap_bytes())
                .sum();
            let members = server.member_groups.heap_bytes()
                + server.member_lists.heap_bytes()
                + server.channel_member_list.heap_bytes()
                + server.member_info.heap_bytes()
                + server.requested_members.heap_bytes()
                + server.known_users.heap_bytes();
            mem.messages += messages;
            mem.members += members;
            mem.structure += server.heap_bytes().saturating_sub(messages + members);
        }

        // Amigos y DMs: los mensajes de cada conversación cuelgan de `Friend`.
        mem.friends_dms += self.friends.capacity() * size_of::<crate::lib::data::Friend>();
        for friend in &self.friends {
            let messages = friend.messages.heap_bytes();
            mem.messages += messages;
            mem.friends_dms += friend.heap_bytes().saturating_sub(messages);
        }
        mem.friends_dms += self.dms.heap_bytes();

        // Cuenta y estado.
        mem.account += self.me.heap_bytes();
        mem.account += self
            .discord_settings
            .as_ref()
            .map(|settings| settings.encoded_len())
            .unwrap_or(0);
        mem.account += self.presences.heap_bytes()
            + self.user_activities.heap_bytes()
            + self.custom_statuses.heap_bytes()
            + self.own_activities.heap_bytes()
            + self.acked_messages.heap_bytes()
            + self.ack_sent_at.heap_bytes()
            + self.unread_mentions.heap_bytes()
            + self.guild_mentions.heap_bytes()
            + self.pending_mentions.heap_bytes()
            + self.activity.heap_bytes();
        mem.account += self
            .toasts
            .iter()
            .map(|t| size_of_val(t) + t.title.capacity() + t.message.capacity())
            .sum::<usize>();
        mem.account += self
            .in_app_notifications
            .iter()
            .map(|n| {
                size_of_val(n) + n.title.capacity() + n.body.capacity() + n.avatar_url.heap_bytes()
            })
            .sum::<usize>();
        // Estado de voz: unos ~200 bytes por participante.
        mem.account += self.voice.participant_count() * 200;

        mem
    }

    /// Arma el informe completo (recorre todos los mensajes: no llamar en cada
    /// frame, ver [`App::memory_report_cached`]).
    pub fn memory_report(&self, ctx: &egui::Context) -> MemoryReport {
        let heap = mem_report::heap_bytes();
        let resident = mem_report::process_memory().map(|m| m.resident);
        let total = resident.unwrap_or(heap);
        let discord = self.discord_memory();
        let images = crate::ui::anim::mem_stats(ctx);
        let video = egui_video::video_mem_stats();
        let measured = discord.total() + images.decoded + images.downloaded + video.rust_heap();
        MemoryReport {
            resident,
            heap,
            total,
            base: total.saturating_sub(measured),
            discord,
            images_decoded: images.decoded,
            images_downloaded: images.downloaded,
            animations: images.animations,
            gpu_total: mem_report::gpu_texture_bytes(ctx),
            video,
            gif_thumbs: crate::ui::gif_thumbs::bytes(),
            native: mem_report::native_breakdown().unwrap_or_default(),
            disk: mem_report::disk_stats(),
        }
    }

    /// Igual que [`App::memory_report`] pero recalculado como mucho una vez
    /// por segundo, y pidiendo un repintado por segundo mientras alguien lo
    /// esté mirando (si nadie lo llama, no se despierta la UI).
    pub fn memory_report_cached(&self, ctx: &egui::Context) -> MemoryReport {
        let id = egui::Id::new("memory_report_cache");
        let now = ctx.input(|i| i.time);
        ctx.request_repaint_after(Duration::from_secs(1));
        if let Some((at, report)) = ctx.data(|d| d.get_temp::<(f64, MemoryReport)>(id)) {
            if now - at < 0.9 {
                return report;
            }
        }
        let report = self.memory_report(ctx);
        ctx.data_mut(|d| d.insert_temp(id, (now, report)));
        report
    }
}
