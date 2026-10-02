//! Una sola instancia de ecord a la vez.
//!
//! La primera instancia abre un socket en `127.0.0.1` (solo local) y un hilo
//! que lo escucha. Si se abre ecord otra vez, la nueva instancia no consigue
//! el puerto: se conecta a ese socket, le pide a la primera que se muestre y
//! se cierra. La primera, al recibir el pedido, restaura la ventana (si
//! estaba minimizada u oculta) y le da el foco.
//!
//! Se eligió un socket local en vez de un mutex/archivo del sistema porque
//! sirve igual en Windows, Linux y macOS, no deja archivos colgados si ecord
//! se cierra mal (el sistema libera el puerto solo) y permite avisarle a la
//! instancia abierta.
//!
//! Si el puerto lo ocupa otro programa (no responde el saludo de ecord) o no
//! se puede abrir por otra razón, ecord arranca igual, sin límite: mejor dos
//! ventanas que no poder abrirlo.

use std::io::{BufRead, BufReader, Write};
use std::net::{Ipv4Addr, SocketAddr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::OnceLock;
use std::time::Duration;

/// Puerto local reservado para esto.
const PORT: u16 = 47913;
/// Pedido que manda la instancia nueva y respuesta de la que ya estaba abierta
/// (seguida de su id de proceso). Sirven de saludo para no confundir a ecord
/// con otro programa que tenga el mismo puerto.
const REQUEST: &str = "ecord-focus";
const REPLY: &str = "ecord-ok";
/// Flag para saltarse el límite (por ejemplo, para probar dos cuentas a la vez).
pub const ALLOW_MULTIPLE_FLAG: &str = "--multi-instance";

fn addr() -> SocketAddr {
    SocketAddr::from((Ipv4Addr::LOCALHOST, PORT))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Role {
    /// Esta es la instancia que sigue: hay que abrir la ventana.
    Primary,
    /// Ya había otra instancia y se le pidió que se muestre: hay que salir.
    Secondary,
}

/// Decide si esta instancia es la primera. Llamar al principio de `main`,
/// antes de crear la ventana.
pub fn acquire() -> Role {
    if std::env::args().any(|arg| arg == ALLOW_MULTIPLE_FLAG) {
        return Role::Primary;
    }
    match TcpListener::bind(addr()) {
        Ok(listener) => {
            spawn_listener(listener);
            Role::Primary
        }
        Err(err) if err.kind() == std::io::ErrorKind::AddrInUse => {
            if ask_running_instance_to_focus() {
                Role::Secondary
            } else {
                log::warn!(
                    "el puerto {PORT} está ocupado por otro programa: ecord arranca sin límite de instancias"
                );
                Role::Primary
            }
        }
        Err(err) => {
            log::warn!("no se pudo abrir el socket de instancia única ({err}): ecord arranca sin límite");
            Role::Primary
        }
    }
}

/// Le avisa a la instancia abierta que se muestre. `true` si respondió ecord.
fn ask_running_instance_to_focus() -> bool {
    let Ok(mut stream) = TcpStream::connect_timeout(&addr(), Duration::from_secs(2)) else {
        return false;
    };
    let _ = stream.set_write_timeout(Some(Duration::from_secs(2)));
    let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
    if writeln!(stream, "{REQUEST}").is_err() {
        return false;
    }
    let mut reply = String::new();
    if BufReader::new(&stream).read_line(&mut reply).is_err() {
        return false;
    }
    let Some(rest) = reply.trim().strip_prefix(REPLY) else {
        return false;
    };
    allow_foreground(rest);
    true
}

/// En Windows, un proceso solo puede traer su ventana al frente si el
/// proceso que tiene el foco se lo permite. Esta instancia (la que acaba de
/// lanzar el usuario) lo tiene, así que se lo cede a la que ya estaba abierta
/// (`reply_rest` es el id de proceso que ella mandó).
#[cfg(windows)]
fn allow_foreground(reply_rest: &str) {
    if let Ok(pid) = reply_rest.trim().parse::<u32>() {
        // SAFETY: llamada simple de la API de Windows, sin punteros.
        unsafe {
            windows_sys::Win32::UI::WindowsAndMessaging::AllowSetForegroundWindow(pid);
        }
    }
}

#[cfg(not(windows))]
fn allow_foreground(_reply_rest: &str) {}

fn spawn_listener(listener: TcpListener) {
    let spawned = std::thread::Builder::new()
        .name("ecord-single-instance".to_owned())
        .spawn(move || {
            for stream in listener.incoming() {
                match stream {
                    Ok(mut stream) => handle_connection(&mut stream),
                    Err(_) => std::thread::sleep(Duration::from_millis(200)),
                }
            }
        });
    if let Err(err) = spawned {
        log::warn!("no se pudo iniciar el hilo de instancia única: {err}");
    }
}

fn handle_connection(stream: &mut TcpStream) {
    let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
    let _ = stream.set_write_timeout(Some(Duration::from_secs(2)));
    let mut line = String::new();
    if BufReader::new(&*stream).read_line(&mut line).is_err() || line.trim() != REQUEST {
        return;
    }
    // Se contesta ya mismo (aunque la ventana todavía no exista) para que la
    // instancia nueva no crea que no hay nadie y abra otra ventana.
    let _ = writeln!(stream, "{REPLY} {}", std::process::id());
    request_focus();
}

// ---------------------------------------------------------------------
// Mostrar la ventana
// ---------------------------------------------------------------------

/// Contexto de egui de la ventana, disponible recién cuando eframe la crea.
static CONTEXT: OnceLock<egui::Context> = OnceLock::new();
/// Pedido de foco que llegó antes de que existiera la ventana.
static PENDING_FOCUS: AtomicBool = AtomicBool::new(false);

/// Registra el contexto de la ventana (llamar desde el creador de eframe). Si
/// ya había llegado un pedido de foco mientras la ventana se abría, lo cumple.
pub fn set_context(ctx: &egui::Context) {
    let _ = CONTEXT.set(ctx.clone());
    if PENDING_FOCUS.swap(false, Ordering::SeqCst) {
        focus(ctx);
    }
}

fn request_focus() {
    PENDING_FOCUS.store(true, Ordering::SeqCst);
    if let Some(ctx) = CONTEXT.get() {
        if PENDING_FOCUS.swap(false, Ordering::SeqCst) {
            focus(ctx);
        }
    }
}

/// Restaura la ventana (minimizada u oculta) y le da el foco.
fn focus(ctx: &egui::Context) {
    ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
    ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(false));
    ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
    ctx.request_repaint();
}
