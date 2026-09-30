//! Orden de la barra de servidores (íconos de la izquierda) y sistema de
//! carpetas, calculado a partir de `PreloadedUserSettings.guild_folders`
//! (ver `discord::user_settings`) — el mismo blob que ordena/agrupa los
//! servers en el cliente oficial.
//!
//! Discord no guarda "el orden" y "las carpetas" como dos cosas separadas:
//! todo vive en un único array, `guild_folders.folders`, que ES el orden
//! de arriba a abajo de la barra. Cada entrada es una de dos cosas:
//! - Una carpeta real (la creó el usuario arrastrando un server sobre
//!   otro): trae `id` propio (y opcionalmente `name`/`color`) y uno o más
//!   `guild_ids`.
//! - Un server "suelto" (no está en ninguna carpeta): una entrada con
//!   exactamente un `guild_id` e `id` en `None` — es solo un placeholder
//!   para que ese server tenga una posición en el array.
//!
//! `guild_positions` (el campo viejo, `#[deprecated]` en el proto) era el
//! orden plano de antes de que existieran las carpetas; no hace falta
//! leerlo si `folders` está presente, así que este módulo lo ignora.

use egui::Color32;

use crate::discord::user_settings::proto::preloaded_user_settings::GuildFolders;
use crate::lib::data::Server;

/// Una entrada de la barra de servidores, ya resuelta a índices dentro de
/// `App::servers`. A propósito NO reordenamos ese `Vec`: un montón de
/// código lo referencia por índice (empezando por `Screen::Server(i)`),
/// así que el "orden real de la barra" vive acá aparte, y se recalcula
/// cada frame a partir de `App::discord_settings` — es barato (unos
/// pocos cientos de servers como mucho) y evita tener que mantener dos
/// fuentes de verdad sincronizadas.
#[derive(Clone, Debug)]
pub enum SidebarEntry {
    /// Server suelto, fuera de cualquier carpeta.
    Guild(usize),
    Folder(SidebarFolder),
}

#[derive(Clone, Debug)]
pub struct SidebarFolder {
    /// Id de la carpeta (`GuildFolder::id`). Sirve como key estable para
    /// `App::open_guild_folders` (abierta/cerrada) entre frames.
    pub id: i64,
    pub name: Option<String>,
    pub color: Option<Color32>,
    /// Índices en `App::servers`, en el orden en que están dentro de la
    /// carpeta. Un `guild_id` de la carpeta que este cliente todavía no
    /// tiene cargado (guild unavailable, o quedó una referencia vieja) se
    /// filtra calladamente en vez de romper el resto del orden.
    pub guild_indices: Vec<usize>,
}

/// Arma el orden de la barra: recorre `guild_folders.folders` colocando
/// cada guild/carpeta, y al final agrega — en su orden original de
/// `servers` (el de `READY`) — cualquier server que no haya aparecido en
/// ningún lado (cuenta sin blob de settings todavía, o un server al que
/// te uniste después de la última sincronización de `guild_folders`).
pub fn build_sidebar_order(
    servers: &[Server],
    guild_folders: Option<&GuildFolders>,
) -> Vec<SidebarEntry> {
    let by_id: std::collections::HashMap<u64, usize> = servers
        .iter()
        .enumerate()
        .filter_map(|(i, s)| s.guild_id.parse::<u64>().ok().map(|id| (id, i)))
        .collect();

    let mut placed = vec![false; servers.len()];
    let mut order = Vec::new();

    if let Some(guild_folders) = guild_folders {
        for folder in &guild_folders.folders {
            let indices: Vec<usize> = folder
                .guild_ids
                .iter()
                .filter_map(|id| by_id.get(id).copied())
                .collect();
            if indices.is_empty() {
                // Ningún guild de esta entrada está cargado acá (todos
                // unavailable, o la carpeta quedó vacía) — no ocupa lugar
                // en la barra.
                continue;
            }
            for &i in &indices {
                placed[i] = true;
            }

            // Placeholder de server suelto: sin `id` de carpeta y un solo
            // guild. Cualquier otra cosa con `id` presente es una carpeta
            // real, aunque le quede un solo server adentro después de
            // filtrar los que no cargamos.
            match (folder.id, indices.as_slice()) {
                (None, [single]) => order.push(SidebarEntry::Guild(*single)),
                _ => order.push(SidebarEntry::Folder(SidebarFolder {
                    id: folder.id.unwrap_or_default(),
                    name: folder.name.clone(),
                    color: folder.color.map(color_from_u64),
                    guild_indices: indices,
                })),
            }
        }
    }

    // Lo que quedó sin colocar: en orden original, al final. Si no hay
    // `guild_folders` todavía, `placed` queda todo en `false` y esto
    // termina agregando todos los servers en su orden de `READY` — mismo
    // comportamiento que había antes de que existiera este módulo.
    for (i, was_placed) in placed.iter().enumerate() {
        if !was_placed {
            order.push(SidebarEntry::Guild(i));
        }
    }

    order
}

/// El `color` de una carpeta es un entero de 24 bits empaquetado (mismo
/// formato que el color de un rol de Discord), sin canal alpha.
fn color_from_u64(color: u64) -> Color32 {
    let color = color as u32;
    Color32::from_rgb(
        ((color >> 16) & 0xFF) as u8,
        ((color >> 8) & 0xFF) as u8,
        (color & 0xFF) as u8,
    )
}
