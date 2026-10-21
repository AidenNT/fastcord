//! Audio de la transmisión (el "sonido" del Go Live propio).
//!
//! * Ventana: se captura solo el audio de ese proceso (y sus hijos: un navegador
//!   reproduce desde un proceso auxiliar) con el loopback por proceso de WASAPI. Si la
//!   ventana es del Discord oficial no se captura nada.
//! * Pantalla / escritorio: todo el audio del sistema MENOS el de ecord y el del Discord
//!   oficial. WASAPI solo deja excluir UN árbol de procesos por captura, así que se
//!   listan los procesos que tienen una sesión de audio, se descartan ecord, Discord y
//!   sus parientes, y se mezcla una captura por cada proceso restante (la lista se
//!   revisa cada 500 ms para recoger los que empiezan a sonar). Si no se pueden listar
//!   las sesiones, se cae a "todo el sistema menos ecord".
//!
//! Entrega bloques de 20 ms (960 muestras por canal, estéreo, 48 kHz, `i16`), que es lo
//! que espera `VoiceOpusEncode::encode_20ms_i16`. Solo Windows: en el resto de
//! plataformas `start` devuelve un error y la transmisión sigue solo con video.
//!
//! Escrito contra la documentación de la crate `wasapi` 0.26 y de `windows-sys`, sin
//! poder compilarlo en Windows.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use tokio::sync::mpsc;

use super::screen_capture::CaptureTarget;

/// Muestras `i16` intercaladas de 20 ms estéreo a 48 kHz.
#[cfg_attr(not(windows), allow(dead_code))]
pub(super) const FRAME_SAMPLES: usize = 960 * 2;
/// Bloques de 20 ms que se guardan si el codificador se atrasa (~1 s).
#[cfg_attr(not(windows), allow(dead_code))]
const QUEUE_FRAMES: usize = 50;

/// Captura en marcha. Al soltarla se detiene el hilo de captura.
pub(super) struct StreamAudioCapture {
    pub(super) frames_rx: mpsc::Receiver<Vec<i16>>,
    stop: Arc<AtomicBool>,
}

impl Drop for StreamAudioCapture {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

/// Qué se captura.
#[cfg_attr(not(windows), allow(dead_code))]
#[derive(Clone, Copy, Debug)]
enum AudioSource {
    /// Un proceso y sus hijos (`include_tree = true`), o todo el sistema salvo ese
    /// proceso (`false`).
    Process { pid: u32, include_tree: bool },
    /// Todo el sistema salvo ecord y el Discord oficial (una captura por proceso).
    SystemWithoutDiscord,
}

#[cfg(not(windows))]
pub(super) fn start(_target: &CaptureTarget) -> Result<StreamAudioCapture, String> {
    Err("el audio de la transmisión solo está disponible en Windows".to_owned())
}

#[cfg(windows)]
pub(super) use imp::start;

#[cfg(windows)]
mod imp {
    use std::collections::{HashMap, HashSet, VecDeque};
    use std::time::{Duration, Instant};

    use wasapi::{
        AudioCaptureClient, AudioClient, DeviceEnumerator, Direction, Handle, SampleType, SessionState, StreamMode,
        WaveFormat, initialize_mta,
    };

    use super::*;
    use crate::logging;

    /// Cada cuánto se vuelve a listar quién tiene una sesión de audio.
    const REFRESH_EVERY: Duration = Duration::from_millis(500);
    /// Un proceso solo entra a la mezcla cuando tiene 40 ms acumulados (evita cortes).
    const PRIME_SAMPLES: usize = FRAME_SAMPLES * 2;
    /// Si un proceso acumula más que esto (~100 ms) se tira lo más viejo.
    const MAX_BACKLOG_SAMPLES: usize = FRAME_SAMPLES * 5;

    // -- Procesos ----------------------------------------------------------

    /// Nombre del ejecutable (sin ruta ni `.exe`, en minúsculas) del proceso `pid`.
    fn process_name(pid: u32) -> Option<String> {
        use windows_sys::Win32::Foundation::CloseHandle;
        use windows_sys::Win32::System::Threading::{
            OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION, QueryFullProcessImageNameW,
        };
        // SAFETY: el handle se abre con permiso mínimo y se cierra acá mismo.
        unsafe {
            let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
            if handle.is_null() {
                return None;
            }
            let mut buffer = [0u16; 520];
            let mut length = buffer.len() as u32;
            let ok = QueryFullProcessImageNameW(handle, 0, buffer.as_mut_ptr(), &mut length);
            CloseHandle(handle);
            if ok == 0 {
                return None;
            }
            let path = String::from_utf16_lossy(&buffer[..length as usize]);
            std::path::Path::new(&path)
                .file_stem()
                .map(|stem| stem.to_string_lossy().to_lowercase())
        }
    }

    /// ¿Es el cliente oficial de Discord (estable, PTB, Canary o desarrollo)?
    fn is_official_discord(pid: u32) -> bool {
        process_name(pid)
            .is_some_and(|name| matches!(name.as_str(), "discord" | "discordptb" | "discordcanary" | "discorddevelopment"))
    }

    /// `pid -> pid del padre` de todos los procesos del sistema.
    fn process_parents() -> HashMap<u32, u32> {
        use windows_sys::Win32::Foundation::{CloseHandle, INVALID_HANDLE_VALUE};
        use windows_sys::Win32::System::Diagnostics::ToolHelp::{
            CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW, TH32CS_SNAPPROCESS,
        };
        let mut parents = HashMap::new();
        // SAFETY: la estructura se inicializa en cero con `dwSize` puesto, como pide la API,
        // y el snapshot se cierra al final.
        unsafe {
            let snapshot = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
            if snapshot == INVALID_HANDLE_VALUE || snapshot.is_null() {
                return parents;
            }
            let mut entry: PROCESSENTRY32W = std::mem::zeroed();
            entry.dwSize = std::mem::size_of::<PROCESSENTRY32W>() as u32;
            let mut ok = Process32FirstW(snapshot, &mut entry);
            while ok != 0 {
                parents.insert(entry.th32ProcessID, entry.th32ParentProcessID);
                ok = Process32NextW(snapshot, &mut entry);
            }
            CloseHandle(snapshot);
        }
        parents
    }

    /// Los ancestros de `pid` (sin incluirlo), del padre hacia arriba.
    fn ancestors(pid: u32, parents: &HashMap<u32, u32>) -> Vec<u32> {
        let mut out = Vec::new();
        let mut current = pid;
        // Los pid se reciclan: el tope evita ciclos raros.
        while out.len() < 32 {
            let Some(&parent) = parents.get(&current) else { break };
            if parent == 0 || parent == current || out.contains(&parent) {
                break;
            }
            out.push(parent);
            current = parent;
        }
        out
    }

    /// PID de los procesos con una sesión de audio en el dispositivo de salida
    /// predeterminado.
    fn audio_session_pids() -> Result<Vec<u32>, String> {
        let enumerator = DeviceEnumerator::new().map_err(|error| format!("{error:?}"))?;
        let device = enumerator
            .get_default_device(&Direction::Render)
            .map_err(|error| format!("{error:?}"))?;
        let manager = device.get_iaudiosessionmanager().map_err(|error| format!("{error:?}"))?;
        let sessions = manager.get_audiosessionenumerator().map_err(|error| format!("{error:?}"))?;
        let count = sessions.get_count().map_err(|error| format!("{error:?}"))?;
        let mut pids = Vec::new();
        for index in 0..count {
            let Ok(control) = sessions.get_session(index) else { continue };
            let Ok(state) = control.get_state() else { continue };
            if state == SessionState::Expired {
                continue;
            }
            if let Ok(pid) = control.get_process_id() {
                // PID 0 son los sonidos del sistema.
                if pid != 0 {
                    pids.push(pid);
                }
            }
        }
        pids.sort_unstable();
        pids.dedup();
        Ok(pids)
    }

    /// De los procesos con sesión de audio, los que hay que capturar: sin ecord ni el
    /// Discord oficial, sin sus hijos, sin sus ancestros (su árbol los incluiría) y sin
    /// los que ya están dentro del árbol de otro de la lista (se contarían dos veces).
    fn capture_roots(pids: &[u32]) -> Vec<u32> {
        let parents = process_parents();
        let own = std::process::id();
        let mut excluded: HashSet<u32> = pids.iter().copied().filter(|pid| is_official_discord(*pid)).collect();
        excluded.insert(own);
        let banned_ancestors: HashSet<u32> = excluded.iter().flat_map(|pid| ancestors(*pid, &parents)).collect();
        let candidates: Vec<u32> = pids
            .iter()
            .copied()
            .filter(|pid| {
                !excluded.contains(pid)
                    && !banned_ancestors.contains(pid)
                    && !ancestors(*pid, &parents).iter().any(|ancestor| excluded.contains(ancestor))
            })
            .collect();
        let set: HashSet<u32> = candidates.iter().copied().collect();
        candidates
            .into_iter()
            .filter(|pid| !ancestors(*pid, &parents).iter().any(|ancestor| set.contains(ancestor)))
            .collect()
    }

    // -- Capturas ----------------------------------------------------------

    /// Una captura por proceso con sus muestras ya convertidas a `i16`.
    struct Loopback {
        client: AudioClient,
        capture: AudioCaptureClient,
        event: Handle,
        raw: VecDeque<u8>,
        pcm: VecDeque<i16>,
        primed: bool,
    }

    impl Loopback {
        fn open(pid: u32, include_tree: bool) -> Result<Self, String> {
            let mut client = AudioClient::new_application_loopback_client(pid, include_tree)
                .map_err(|error| format!("loopback de audio por proceso: {error:?}"))?;
            // f32 estéreo a 48 kHz; `autoconvert` deja que Windows convierta del formato del mezclador.
            let format = WaveFormat::new(32, 32, &SampleType::Float, 48_000, 2, None);
            let mode = StreamMode::EventsShared { autoconvert: true, buffer_duration_hns: 200_000 };
            client
                .initialize_client(&format, &Direction::Capture, &mode)
                .map_err(|error| format!("no se pudo iniciar la captura de audio: {error:?}"))?;
            let event = client
                .set_get_eventhandle()
                .map_err(|error| format!("audio: sin evento de captura: {error:?}"))?;
            let capture = client
                .get_audiocaptureclient()
                .map_err(|error| format!("audio: sin cliente de captura: {error:?}"))?;
            client
                .start_stream()
                .map_err(|error| format!("no se pudo arrancar la captura de audio: {error:?}"))?;
            Ok(Self { client, capture, event, raw: VecDeque::new(), pcm: VecDeque::new(), primed: false })
        }

        /// Pasa a `pcm` todo lo que Windows tenga listo.
        fn pump(&mut self) -> Result<(), String> {
            self.capture
                .read_from_device_to_deque(&mut self.raw)
                .map_err(|error| format!("{error:?}"))?;
            while self.raw.len() >= 4 {
                let raw = [
                    self.raw.pop_front().unwrap_or(0),
                    self.raw.pop_front().unwrap_or(0),
                    self.raw.pop_front().unwrap_or(0),
                    self.raw.pop_front().unwrap_or(0),
                ];
                let sample = f32::from_le_bytes(raw).clamp(-1.0, 1.0);
                self.pcm.push_back((sample * 32767.0) as i16);
            }
            Ok(())
        }
    }

    impl Drop for Loopback {
        fn drop(&mut self) {
            let _ = self.client.stop_stream();
        }
    }

    // -- Arranque ----------------------------------------------------------

    fn source_for(target: &CaptureTarget) -> Result<AudioSource, String> {
        match target {
            CaptureTarget::Window { hwnd, .. } => {
                let mut pid: u32 = 0;
                // SAFETY: `hwnd` es un manejador de ventana; si ya no existe, `pid` queda en 0.
                unsafe {
                    windows_sys::Win32::UI::WindowsAndMessaging::GetWindowThreadProcessId(
                        *hwnd as windows_sys::Win32::Foundation::HWND,
                        &mut pid,
                    );
                }
                if pid == 0 {
                    return Ok(AudioSource::SystemWithoutDiscord);
                }
                // Ventana del Discord oficial: nunca se captura su audio.
                if is_official_discord(pid) {
                    return Err("la ventana es del Discord oficial: no se captura su audio".to_owned());
                }
                Ok(AudioSource::Process { pid, include_tree: true })
            }
            _ => Ok(AudioSource::SystemWithoutDiscord),
        }
    }

    pub(in super::super) fn start(target: &CaptureTarget) -> Result<StreamAudioCapture, String> {
        let source = source_for(target)?;
        let stop = Arc::new(AtomicBool::new(false));
        let (frames_tx, frames_rx) = mpsc::channel::<Vec<i16>>(QUEUE_FRAMES);
        let (ready_tx, ready_rx) = std::sync::mpsc::sync_channel::<Result<(), String>>(1);
        let thread_stop = Arc::clone(&stop);
        std::thread::Builder::new()
            .name("stream-audio-capture".to_owned())
            .spawn(move || capture_thread(source, &thread_stop, &frames_tx, &ready_tx))
            .map_err(|error| format!("no se pudo crear el hilo de audio: {error}"))?;
        match ready_rx.recv_timeout(Duration::from_secs(5)) {
            Ok(Ok(())) => Ok(StreamAudioCapture { frames_rx, stop }),
            Ok(Err(error)) => Err(error),
            Err(_) => {
                stop.store(true, Ordering::Relaxed);
                Err("la captura de audio no arrancó en 5 s".to_owned())
            }
        }
    }

    /// `false` si el receptor ya no existe (hay que terminar).
    fn send_frame(frames_tx: &mpsc::Sender<Vec<i16>>, frame: Vec<i16>) -> bool {
        // Cola llena: se pierde este bloque en vez de acumular retraso.
        !matches!(frames_tx.try_send(frame), Err(mpsc::error::TrySendError::Closed(_)))
    }

    fn capture_thread(
        source: AudioSource,
        stop: &AtomicBool,
        frames_tx: &mpsc::Sender<Vec<i16>>,
        ready_tx: &std::sync::mpsc::SyncSender<Result<(), String>>,
    ) {
        let _ = initialize_mta();
        match source {
            AudioSource::Process { pid, include_tree } => match Loopback::open(pid, include_tree) {
                Ok(loopback) => {
                    let _ = ready_tx.send(Ok(()));
                    single_loop(loopback, stop, frames_tx);
                }
                Err(error) => {
                    let _ = ready_tx.send(Err(error));
                }
            },
            AudioSource::SystemWithoutDiscord => {
                // Si no se pueden listar las sesiones, se cae a "todo menos ecord".
                if audio_session_pids().is_ok() {
                    let _ = ready_tx.send(Ok(()));
                    mixed_loop(stop, frames_tx);
                } else {
                    logging::debug("stream", "audio: sin lista de sesiones; se captura todo el sistema menos ecord");
                    match Loopback::open(std::process::id(), false) {
                        Ok(loopback) => {
                            let _ = ready_tx.send(Ok(()));
                            single_loop(loopback, stop, frames_tx);
                        }
                        Err(error) => {
                            let _ = ready_tx.send(Err(error));
                        }
                    }
                }
            }
        }
    }

    /// Una sola captura: cada bloque de 20 ms sale tal cual llega.
    fn single_loop(mut loopback: Loopback, stop: &AtomicBool, frames_tx: &mpsc::Sender<Vec<i16>>) {
        while !stop.load(Ordering::Relaxed) {
            // Sin sonido reproduciéndose no hay eventos: el timeout deja revisar `stop`.
            let _ = loopback.event.wait_for_event(100);
            if loopback.pump().is_err() {
                return;
            }
            while loopback.pcm.len() >= FRAME_SAMPLES {
                let frame: Vec<i16> = loopback.pcm.drain(..FRAME_SAMPLES).collect();
                if !send_frame(frames_tx, frame) {
                    return;
                }
            }
        }
    }

    /// Una captura por proceso que suena (menos ecord y Discord), mezcladas en bloques de
    /// 20 ms con el reloj de la pared.
    fn mixed_loop(stop: &AtomicBool, frames_tx: &mpsc::Sender<Vec<i16>>) {
        let tick = Duration::from_millis(20);
        let mut sources: HashMap<u32, Loopback> = HashMap::new();
        let mut failed: HashSet<u32> = HashSet::new();
        let mut last_refresh = Instant::now().checked_sub(REFRESH_EVERY).unwrap_or_else(Instant::now);
        let mut next_tick = Instant::now() + tick;

        while !stop.load(Ordering::Relaxed) {
            let now = Instant::now();
            if now.saturating_duration_since(last_refresh) >= REFRESH_EVERY {
                last_refresh = now;
                if let Ok(pids) = audio_session_pids() {
                    let roots = capture_roots(&pids);
                    sources.retain(|pid, _| roots.contains(pid));
                    failed.retain(|pid| roots.contains(pid));
                    for pid in roots {
                        if sources.contains_key(&pid) || failed.contains(&pid) {
                            continue;
                        }
                        match Loopback::open(pid, true) {
                            Ok(loopback) => {
                                sources.insert(pid, loopback);
                            }
                            Err(error) => {
                                logging::debug("stream", format!("audio: no se capturó el proceso {pid}: {error}"));
                                failed.insert(pid);
                            }
                        }
                    }
                }
            }

            // Lo que Windows tenga listo; las capturas que fallan se sueltan.
            sources.retain(|_, source| source.pump().is_ok());

            while Instant::now() >= next_tick {
                next_tick += tick;
                let mut mix = vec![0i32; FRAME_SAMPLES];
                let mut any = false;
                for source in sources.values_mut() {
                    while source.pcm.len() > MAX_BACKLOG_SAMPLES {
                        source.pcm.drain(..FRAME_SAMPLES);
                    }
                    if !source.primed {
                        if source.pcm.len() < PRIME_SAMPLES {
                            continue;
                        }
                        source.primed = true;
                    }
                    if source.pcm.len() < FRAME_SAMPLES {
                        // Terminó de sonar: se descarta la cola y se vuelve a esperar.
                        source.pcm.clear();
                        source.primed = false;
                        continue;
                    }
                    for (slot, sample) in mix.iter_mut().zip(source.pcm.drain(..FRAME_SAMPLES)) {
                        *slot += i32::from(sample);
                    }
                    any = true;
                }
                if any {
                    let frame: Vec<i16> = mix
                        .into_iter()
                        .map(|value| value.clamp(i32::from(i16::MIN), i32::from(i16::MAX)) as i16)
                        .collect();
                    if !send_frame(frames_tx, frame) {
                        return;
                    }
                }
                // Si el hilo se durmió mucho, no se emiten ráfagas de bloques atrasados.
                let now = Instant::now();
                if next_tick + Duration::from_millis(200) < now {
                    next_tick = now;
                }
            }
            std::thread::sleep(Duration::from_millis(2));
        }
    }
}
