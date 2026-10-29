//! eCord como app residente: sigue corriendo sin ventana (en la bandeja).
//!
//! El loop vive en `main` (`fastframe_shell::Shell`) y el pegamento con la
//! bandeja en `app_shell`. Acá está lo que necesita el estado de la app:
//!
//! - [`Resident`]: qué hacer cuando la ventana se cierra (esconder a la
//!   bandeja o salir) y qué trabajo de fondo corre en cada tick sin ventana.
//! - [`App::window_logic`]: lo que se decide con la ventana abierta (cierre =
//!   esconder, clics de la bandeja, pedidos de mostrar de otra instancia o de
//!   una notificación). Va en `logic` y no en `ui`: eframe no llama a `ui`
//!   con la ventana minimizada, y entonces un cierre desde la barra de tareas
//!   saldría de eCord en vez de esconderlo.

use std::collections::HashMap;
use std::time::Duration;

use fastframe_shell::{Closed, Headless, Resident};
use fastframe_tray::{Event, Tray};

use super::{App, AuthStatus, Screen};
use crate::app_shell::{self, MENU_QUIT, MENU_SHOW};
use crate::theme::BackdropRuntime;

/// Cada cuánto corre un tick sin ventana estando conectado. Igual que el
/// repintado de respaldo de la ventana: el hilo de Discord no despierta a la UI.
const CONNECTED_TICK: Duration = Duration::from_millis(500);
/// Sin sesión no hay nada que atender: el proceso casi no se despierta.
const IDLE_TICK: Duration = Duration::from_secs(2);
/// Tiempo máximo procesando eventos del Gateway por frame con ventana: deja
/// margen para dibujar (16 ms a 60 fps) aunque haya una cola grande.
const WINDOW_EVENT_BUDGET: Duration = Duration::from_millis(8);
/// Sin ventana no hay nada que dibujar: se puede procesar más por tick.
const HEADLESS_EVENT_BUDGET: Duration = Duration::from_millis(40);
/// Cuando quedan eventos en la cola, el próximo tick sin ventana va casi ya.
const BACKLOG_TICK: Duration = Duration::from_millis(5);
/// Clave (en el almacenamiento local) del aviso "corre en segundo plano".
const BACKGROUND_NOTICE_KEY: &str = "background_notice_shown";

/// Estado de la bandeja y de lo que decide qué pasa al cerrar la ventana.
#[derive(Default)]
pub struct ShellState {
    tray: Option<Tray>,
    /// El cierre de la ventana en curso es "esconder a la bandeja".
    hide_intent: bool,
    /// Abrir una ventana nueva ya mismo (Wayland no deja enfocar la propia).
    reopen: bool,
    /// Un clic de la bandeja o un pedido externo quiere la ventana.
    wants_show: bool,
    /// Servers con cambios de canal anotados que faltan rearmar, con el canal
    /// que se estaba mirando ANTES del primer cambio (ver `flush_channel_rebuilds`).
    channel_rebuilds: HashMap<String, Option<String>>,
}

impl App {
    /// Crea la bandeja. Llamar una vez, antes de entregar el `App` al `Shell`.
    pub fn install_tray(&mut self) {
        self.shell.tray = app_shell::spawn_tray();
    }

    /// Llamar al crear cada ventana. En macOS la primera llamada crea el ítem
    /// de la barra de menú y cada una trae la app al frente.
    pub fn attach_window(&mut self) {
        self.shell.hide_intent = false;
        self.shell.reopen = false;
        self.shell.wants_show = false;
        if let Some(tray) = self.shell.tray.as_mut() {
            tray.attach();
        }
    }

    /// ¿Cerrar la ventana la esconde en vez de salir? Solo con bandeja visible
    /// (si no, no habría cómo volver), con sesión y sin un pedido de salir.
    /// `ECORD_QUIT_ON_CLOSE=1` lo apaga: cerrar la ventana cierra eCord.
    fn hides_to_tray(&self) -> bool {
        !app_shell::quit_requested()
            && std::env::var_os("ECORD_QUIT_ON_CLOSE").is_none()
            && !matches!(self.screen, Screen::Login)
            && self.shell.tray.as_ref().is_some_and(Tray::is_shown)
    }

    /// Marca que `guild_id` tiene cambios de canal pendientes. Guarda el canal
    /// abierto la primera vez, antes de que los índices se corran.
    pub(super) fn queue_channel_rebuild(&mut self, guild_id: &str) {
        if !self.shell.channel_rebuilds.contains_key(guild_id) {
            let previous = self.current_channel_id();
            self.shell.channel_rebuilds.insert(guild_id.to_owned(), previous);
        }
    }

    /// Rearma las categorías de cada server con cambios pendientes, una vez
    /// por server, y vuelve a ubicar el canal que se estaba mirando.
    pub(super) fn flush_channel_rebuilds(&mut self) {
        if self.shell.channel_rebuilds.is_empty() {
            return;
        }
        for (guild_id, previous) in std::mem::take(&mut self.shell.channel_rebuilds) {
            if let Some(server) = self.servers.iter_mut().find(|s| s.guild_id == guild_id) {
                server.rebuild_staged_channels();
            }
            self.restore_current_channel(&guild_id, previous);
        }
    }

    /// Avisa UNA vez en la vida de la instalación que cerrar la ventana no
    /// cierra eCord: si no, parece que se cerró y que no llegan mensajes.
    fn announce_background_once(&self) {
        let shown = matches!(web_local_storage_api::get_item(BACKGROUND_NOTICE_KEY), Ok(Some(_)));
        if shown {
            return;
        }
        let _ = web_local_storage_api::set_item(BACKGROUND_NOTICE_KEY, "true");
        crate::lib::notifications::send_desktop(
            "eCord está corriendo en segundo plano".into(),
            "Seguís conectado y vas a recibir notificaciones. Abrilo o salí desde el ícono de la bandeja.".into(),
        );
    }

    fn tray_events(&self) -> Vec<Event> {
        self.shell.tray.as_ref().map(Tray::events).unwrap_or_default()
    }

    /// Lo que se decide cada vez que corre la ventana (también minimizada).
    pub fn window_logic(&mut self, ctx: &egui::Context) {
        if ctx.input(|i| i.viewport().close_requested()) && self.hides_to_tray() {
            self.shell.hide_intent = true;
            self.announce_background_once();
        }

        // Foco real: una ventana minimizada puede no reportar `focused`, y
        // contarla como enfocada mandaba el aviso a una tarjeta dentro de la
        // app que nadie ve en vez de a una notificación del escritorio.
        self.window_focused = ctx.input(|i| {
            i.viewport().focused.unwrap_or(true) && !i.viewport().minimized.unwrap_or(false)
        });
        // Sin foco durante un rato => ausente (opcode 3, idle + afk); al
        // volver, en línea otra vez.
        self.tick_presence(ctx);
        // Eventos del Gateway: acá y no en `ui`, que eframe salta si la
        // ventana está minimizada u oculta (sin esto, sin notificaciones).
        if self.poll_discord_events(ctx, WINDOW_EVENT_BUDGET) {
            ctx.request_repaint();
        }
        self.trim_inactive_history(app_shell::uptime_secs());

        for event in self.tray_events() {
            match event {
                Event::Toggle | Event::Menu(MENU_SHOW) => {
                    let (minimized, focused) =
                        ctx.input(|i| (i.viewport().minimized.unwrap_or(false), i.viewport().focused.unwrap_or(true)));
                    if !minimized && focused && self.hides_to_tray() {
                        self.shell.hide_intent = true;
                        self.announce_background_once();
                        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                    } else {
                        self.raise(ctx);
                    }
                }
                Event::Show => self.raise(ctx),
                Event::Menu(MENU_QUIT) => {
                    app_shell::request_quit();
                    ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                }
                Event::Menu(_) => {}
            }
        }

        // Segunda instancia o clic en una notificación.
        if app_shell::take_show_request() {
            self.raise(ctx);
        }
    }

    /// Trae la ventana al frente.
    fn raise(&mut self, ctx: &egui::Context) {
        // Wayland ignora el pedido de foco de la propia ventana: salvo que el
        // compositor (niri) la enfoque, se cierra y el `Shell` abre una nueva,
        // que aparece en el espacio de trabajo actual.
        #[cfg(target_os = "linux")]
        if std::env::var_os("WAYLAND_DISPLAY").is_some()
            && !ctx.input(|i| i.viewport().focused.unwrap_or(true))
            && !fastframe_shell::window::raise_on_compositor()
        {
            self.shell.reopen = true;
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            return;
        }
        ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
        ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(false));
        ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
        ctx.request_repaint();
    }
}

impl Resident for App {
    fn closed(&self) -> Closed {
        if app_shell::quit_requested() {
            Closed::Quit
        } else if self.shell.reopen {
            Closed::Reopen
        } else if self.shell.hide_intent {
            Closed::Hide
        } else {
            Closed::Quit
        }
    }

    /// La ventana ya no existe y eCord sigue en segundo plano. Se suelta todo
    /// lo que era del contexto de esa ventana: sus texturas no sirven en la
    /// ventana siguiente (que tendrá otro `egui::Context`).
    fn window_gone(&mut self) {
        self.shell.hide_intent = false;
        self.shell.wants_show = false;
        self.window_focused = false;
        self.backdrop_rt = BackdropRuntime::default();
        if let Some(broadcast) = self.broadcasting_stream.as_mut() {
            broadcast.preview_texture = None;
        }
        if let Some(watched) = self.watching_stream.as_mut() {
            watched.texture = None;
        }
        crate::ui::chat::gif_thumbs::clear();
        // Un contexto nuevo arranca con zoom 1: que `ui` vuelva a aplicar el
        // tamaño elegido en vez de tomar ese 1 como si lo hubiera cambiado el usuario.
        self.ui_scale_applied = f32::NAN;
    }

    /// Un tick sin ventana: lo que `ui` hace por frame y no necesita dibujar.
    fn headless_frame(&mut self, ctx: &egui::Context) -> Headless {
        self.window_focused = false;
        crate::support::mem_report::release_free_memory_periodic();
        // Eventos del gateway: contadores, notificaciones, llamadas entrantes.
        let backlog = self.poll_discord_events(ctx, HEADLESS_EVENT_BUDGET);
        // Sin esto el historial de los chats crecía sin tope mientras eCord
        // estaba en la bandeja (solo se recortaba con la ventana dibujándose).
        self.trim_inactive_history(app_shell::uptime_secs());

        for event in self.tray_events() {
            match event {
                Event::Toggle | Event::Show | Event::Menu(MENU_SHOW) => self.shell.wants_show = true,
                Event::Menu(MENU_QUIT) => app_shell::request_quit(),
                Event::Menu(_) => {}
            }
        }
        if app_shell::take_show_request() {
            self.shell.wants_show = true;
        }

        if app_shell::quit_requested() {
            Headless::Quit
        } else if self.shell.wants_show {
            Headless::Show
        } else if backlog {
            Headless::WaitFor(BACKLOG_TICK)
        } else if matches!(self.auth, AuthStatus::Connected) {
            Headless::WaitFor(CONNECTED_TICK)
        } else {
            Headless::WaitFor(IDLE_TICK)
        }
    }

    /// Arranque sin ventana (`--start-hidden`). Se rechaza si no habría cómo
    /// volver (sin bandeja visible) o no hay sesión que mantener.
    fn start_hidden(&mut self) -> bool {
        if !self.hides_to_tray() {
            return false;
        }
        self.shell.hide_intent = true;
        // macOS: el ítem de la barra de menú, sin traer la app al frente.
        if let Some(tray) = self.shell.tray.as_mut() {
            tray.create_item();
        }
        true
    }

    fn shutdown(&mut self) {
        crate::lib::last_view::flush();
    }
}