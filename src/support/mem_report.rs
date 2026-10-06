//! Contador de memoria: de dónde sale la RAM y la caché de ecord.
//!
//! Junta cuatro fuentes y las expone para `ui::memory_view` (pestaña
//! Ajustes → Memoria) y para la píldora "RAM" de `ui::topbar`:
//!
//! 1. **RAM real del proceso** que informa el sistema operativo
//!    ([`process_memory`]): *working set* en Windows, `VmRSS` en Linux.
//! 2. **Heap de Rust vivo**, exacto, gracias a [`CountingAlloc`] (envuelve a
//!    `mimalloc`, ver `main.rs`): suma lo que se pidió y resta lo que se liberó.
//! 3. **Tamaño estimado de los datos de Discord** (mensajes, canales,
//!    miembros...) con el trait [`HeapBytes`]. Es una ESTIMACIÓN: cuenta el
//!    tamaño de los structs, la capacidad de los `Vec`/`String` y una
//!    sobrecarga aproximada de los `HashMap`, pero no la fragmentación del
//!    allocator.
//! 4. **Caché en disco** ([`disk_stats`]): se mide en un hilo aparte, nunca en
//!    el de la UI.
//!
//! "Funcionamiento" (el programa, la UI, fuentes, audio/voz, ffmpeg...) no se
//! mide directo: es lo que queda de la RAM del proceso después de restar todo
//! lo anterior (ver `lib::state::memory`).

use std::alloc::{GlobalAlloc, Layout};
use std::collections::{HashMap, HashSet, VecDeque};
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicIsize, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use mimalloc::MiMalloc;

// ---------------------------------------------------------------------
// Allocator que cuenta
// ---------------------------------------------------------------------

/// Bytes pedidos al allocator y todavía no liberados. `isize` para que un
/// desbalance puntual nunca dé la vuelta a un número gigante.
static HEAP_LIVE: AtomicIsize = AtomicIsize::new(0);

/// `mimalloc` con un contador de bytes vivos. Se registra como
/// `#[global_allocator]` en `main.rs`. El costo es un `fetch_add` relajado por
/// asignación, despreciable frente a lo que ya cuesta pedir memoria.
pub struct CountingAlloc;

unsafe impl GlobalAlloc for CountingAlloc {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let ptr = unsafe { MiMalloc.alloc(layout) };
        if !ptr.is_null() {
            HEAP_LIVE.fetch_add(layout.size() as isize, Ordering::Relaxed);
        }
        ptr
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { MiMalloc.dealloc(ptr, layout) };
        HEAP_LIVE.fetch_sub(layout.size() as isize, Ordering::Relaxed);
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        let ptr = unsafe { MiMalloc.alloc_zeroed(layout) };
        if !ptr.is_null() {
            HEAP_LIVE.fetch_add(layout.size() as isize, Ordering::Relaxed);
        }
        ptr
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        let new_ptr = unsafe { MiMalloc.realloc(ptr, layout, new_size) };
        if !new_ptr.is_null() {
            HEAP_LIVE.fetch_add(new_size as isize - layout.size() as isize, Ordering::Relaxed);
        }
        new_ptr
    }
}

/// Pide a mimalloc que libere al sistema toda la memoria libre que pueda.
/// Barata si no hay nada que soltar; se llama cada tanto desde la UI.
pub fn release_free_memory() {
    unsafe { libmimalloc_sys::mi_collect(true) };
}

/// Compacta los heaps de Windows del proceso (`HeapCompact`). `mimalloc` solo
/// maneja lo que pide Rust; el `malloc` de ffmpeg y de los drivers vive en los
/// heaps nativos y, tras cerrar un video, queda fragmentado y sin devolver.
/// Esto no libera nada que siga en uso: solo junta bloques libres contiguos y
/// le da al sistema las páginas vacías. Se hace en un hilo aparte porque con
/// heaps grandes puede tardar unos milisegundos.
#[cfg(all(windows, target_pointer_width = "64"))]
fn compact_native_heaps_now() {
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetProcessHeaps(count: u32, heaps: *mut isize) -> u32;
        fn HeapCompact(heap: isize, flags: u32) -> usize;
    }
    let mut heaps = [0isize; 64];
    let n = unsafe { GetProcessHeaps(heaps.len() as u32, heaps.as_mut_ptr()) } as usize;
    for &heap in &heaps[..n.min(heaps.len())] {
        unsafe { HeapCompact(heap, 0) };
    }
}

/// Saca del working set las páginas que el proceso no está usando
/// (`EmptyWorkingSet`). No libera nada: las páginas vuelven solas, desde el
/// archivo de paginación o a cero, si algo las toca. Sin esto Windows deja en
/// el working set lo que mimalloc, los drivers de GPU y ffmpeg ya soltaron, y
/// el número que se ve (Administrador de tareas, esta pestaña) no baja nunca.
#[cfg(all(windows, target_pointer_width = "64"))]
fn trim_working_set_now() {
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetCurrentProcess() -> isize;
        fn K32EmptyWorkingSet(process: isize) -> i32;
    }
    unsafe { K32EmptyWorkingSet(GetCurrentProcess()) };
}

#[cfg(not(all(windows, target_pointer_width = "64")))]
fn compact_native_heaps_now() {}
#[cfg(not(all(windows, target_pointer_width = "64")))]
fn trim_working_set_now() {}

/// Compacta los heaps nativos en un hilo aparte.
pub fn compact_native_heaps() {
    use std::sync::atomic::AtomicBool;
    static RUNNING: AtomicBool = AtomicBool::new(false);
    if RUNNING.swap(true, Ordering::AcqRel) {
        return;
    }
    let spawned = std::thread::Builder::new().name("ecord-heap-compact".into()).spawn(|| {
        compact_native_heaps_now();
        RUNNING.store(false, Ordering::Release);
    });
    if spawned.is_err() {
        RUNNING.store(false, Ordering::Release);
    }
}

/// Para después de cerrar algo pesado (el selector de GIFs): espera a que el
/// frame siguiente suelte las texturas de la GPU, compacta los heaps nativos y
/// recorta el working set. A lo sumo uno a la vez; no toca la UI.
pub fn trim_native_later() {
    use std::sync::atomic::AtomicBool;
    static RUNNING: AtomicBool = AtomicBool::new(false);
    if RUNNING.swap(true, Ordering::AcqRel) {
        return;
    }
    let spawned = std::thread::Builder::new().name("ecord-mem-trim".into()).spawn(|| {
        std::thread::sleep(Duration::from_millis(1500));
        compact_native_heaps_now();
        trim_working_set_now();
        RUNNING.store(false, Ordering::Release);
    });
    if spawned.is_err() {
        RUNNING.store(false, Ordering::Release);
    }
}

/// Como [`release_free_memory`], pero a lo sumo una vez cada 20 s.
pub fn release_free_memory_periodic() {
    use std::sync::OnceLock;
    static START: OnceLock<Instant> = OnceLock::new();
    static LAST_SECS: AtomicIsize = AtomicIsize::new(-100);
    let now = START.get_or_init(Instant::now).elapsed().as_secs() as isize;
    if now - LAST_SECS.load(Ordering::Relaxed) >= 20 {
        LAST_SECS.store(now, Ordering::Relaxed);
        release_free_memory();
        compact_native_heaps();
    }
}

/// Bytes de heap de Rust vivos ahora mismo.
pub fn heap_bytes() -> usize {
    HEAP_LIVE.load(Ordering::Relaxed).max(0) as usize
}

// ---------------------------------------------------------------------
// RAM del proceso según el sistema operativo
// ---------------------------------------------------------------------

#[derive(Clone, Copy, Debug, Default)]
pub struct ProcessMemory {
    /// RAM física que ocupa el proceso (working set en Windows, RSS en
    /// Linux). Incluye las páginas de DLLs/librerías compartidas que se
    /// tocaron, así que puede ser algo mayor que la columna "Memoria" del
    /// Administrador de tareas (que muestra solo la parte privada).
    pub resident: usize,
}

#[cfg(windows)]
pub fn process_memory() -> Option<ProcessMemory> {
    // `PROCESS_MEMORY_COUNTERS` de psapi. Se declara a mano (con
    // `K32GetProcessMemoryInfo`, que exporta kernel32 desde Windows 7) para
    // no sumar features a `windows-sys`.
    #[repr(C)]
    #[allow(dead_code)]
    struct Counters {
        cb: u32,
        page_fault_count: u32,
        peak_working_set_size: usize,
        working_set_size: usize,
        quota_peak_paged_pool_usage: usize,
        quota_paged_pool_usage: usize,
        quota_peak_non_paged_pool_usage: usize,
        quota_non_paged_pool_usage: usize,
        pagefile_usage: usize,
        peak_pagefile_usage: usize,
    }

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetCurrentProcess() -> isize;
        fn K32GetProcessMemoryInfo(process: isize, counters: *mut Counters, cb: u32) -> i32;
    }

    let mut counters = Counters {
        cb: std::mem::size_of::<Counters>() as u32,
        page_fault_count: 0,
        peak_working_set_size: 0,
        working_set_size: 0,
        quota_peak_paged_pool_usage: 0,
        quota_paged_pool_usage: 0,
        quota_peak_non_paged_pool_usage: 0,
        quota_non_paged_pool_usage: 0,
        pagefile_usage: 0,
        peak_pagefile_usage: 0,
    };
    let cb = counters.cb;
    let ok = unsafe { K32GetProcessMemoryInfo(GetCurrentProcess(), &mut counters, cb) };
    (ok != 0).then_some(ProcessMemory { resident: counters.working_set_size })
}

#[cfg(target_os = "linux")]
pub fn process_memory() -> Option<ProcessMemory> {
    let status = std::fs::read_to_string("/proc/self/status").ok()?;
    let kb = status
        .lines()
        .find_map(|line| line.strip_prefix("VmRSS:"))?
        .trim()
        .trim_end_matches("kB")
        .trim()
        .parse::<usize>()
        .ok()?;
    Some(ProcessMemory { resident: kb * 1024 })
}

#[cfg(not(any(windows, target_os = "linux")))]
pub fn process_memory() -> Option<ProcessMemory> {
    None
}

/// RAM del proceso para mostrar en la barra superior. Consulta al sistema como
/// mucho una vez por segundo (el resto de las veces devuelve el último valor)
/// y, si el sistema no lo informa, cae al heap de Rust.
pub fn resident_now() -> usize {
    static LAST: Mutex<Option<(Instant, usize)>> = Mutex::new(None);
    let mut last = LAST.lock().unwrap_or_else(|e| e.into_inner());
    if let Some((at, value)) = *last {
        if at.elapsed() < Duration::from_secs(1) {
            return value;
        }
    }
    let value = process_memory().map(|m| m.resident).unwrap_or_else(heap_bytes);
    *last = Some((Instant::now(), value));
    value
}

// ---------------------------------------------------------------------
// Texturas de egui (GPU)
// ---------------------------------------------------------------------

/// Bytes de todas las texturas que `egui` tiene subidas (atlas de fuentes,
/// íconos, imágenes, cuadros de GIF...). Viven en la GPU (en gráficos
/// integrados, en RAM compartida) y NO se cuentan dentro de la RAM del proceso.
pub fn gpu_texture_bytes(ctx: &egui::Context) -> usize {
    let manager = ctx.tex_manager();
    let manager = manager.read();
    manager.allocated().map(|(_, meta)| meta.bytes_used()).sum()
}

// ---------------------------------------------------------------------
// Caché en disco
// ---------------------------------------------------------------------

#[derive(Clone, Copy, Debug, Default)]
pub struct DiskStats {
    /// Imágenes/archivos bajados por HTTP (`support::http_cache`).
    pub images: u64,
    pub image_files: usize,
    /// JSON chicos cacheados (catálogos).
    pub json: u64,
    /// Audios de las alertas sonoras.
    pub sounds: u64,
    /// mp4 de los GIFs ya bajados (`ui::video_player::local_video`).
    pub video: u64,
    /// `false` mientras el primer escaneo no terminó.
    pub scanned: bool,
}

impl DiskStats {
    pub fn total(&self) -> u64 {
        self.images + self.json + self.sounds + self.video
    }
}

/// Último escaneo y cuándo se hizo. El escaneo corre en un hilo aparte;
/// mientras tanto se devuelve el resultado anterior.
static DISK: Mutex<Option<(Instant, DiskStats)>> = Mutex::new(None);
static DISK_SCANNING: AtomicBool = AtomicBool::new(false);
const DISK_RESCAN_EVERY: Duration = Duration::from_secs(10);

pub fn disk_stats() -> DiskStats {
    let (stale, current) = {
        let guard = DISK.lock().unwrap_or_else(|e| e.into_inner());
        match *guard {
            Some((at, stats)) => (at.elapsed() >= DISK_RESCAN_EVERY, stats),
            None => (true, DiskStats::default()),
        }
    };
    if stale && !DISK_SCANNING.swap(true, Ordering::AcqRel) {
        std::thread::spawn(|| {
            let (images, image_files) = crate::paths::http_cache_dir()
                .map(|d| dir_size(&d))
                .unwrap_or((0, 0));
            let (json, _) = crate::paths::json_cache_dir().map(|d| dir_size(&d)).unwrap_or((0, 0));
            let (sounds, _) = crate::paths::sound_cache_dir().map(|d| dir_size(&d)).unwrap_or((0, 0));
            let (video, _) = crate::paths::video_cache_dir().map(|d| dir_size(&d)).unwrap_or((0, 0));
            let stats = DiskStats { images, image_files, json, sounds, video, scanned: true };
            *DISK.lock().unwrap_or_else(|e| e.into_inner()) = Some((Instant::now(), stats));
            DISK_SCANNING.store(false, Ordering::Release);
        });
    }
    current
}

/// Tamaño total y cantidad de archivos de una carpeta (las de caché son
/// planas: un archivo por URL).
fn dir_size(dir: &Path) -> (u64, usize) {
    let Ok(read) = std::fs::read_dir(dir) else { return (0, 0) };
    let mut total = 0u64;
    let mut files = 0usize;
    for entry in read.flatten() {
        if let Ok(meta) = entry.metadata() {
            if meta.is_file() {
                total += meta.len();
                files += 1;
            }
        }
    }
    (total, files)
}

// ---------------------------------------------------------------------
// Desglose de la RAM que NO es heap de Rust (Windows)
// ---------------------------------------------------------------------

/// A qué pertenece la RAM residente del proceso, página por página.
///
/// Se arma con `QueryWorkingSet` (qué páginas están en RAM) cruzado con
/// `VirtualQuery` (de qué tipo es cada región) y la lista de módulos (a qué
/// DLL pertenece cada página de código/datos). Todos los números son bytes
/// residentes, o sea lo mismo que cuenta el working set.
#[derive(Clone, Copy, Debug, Default)]
pub struct NativeBreakdown {
    /// Memoria privada (`MEM_PRIVATE`): heap de Rust + heaps nativos (el
    /// `malloc` de ffmpeg y de los drivers) + pilas de los hilos.
    pub private: usize,
    /// Memoria mapeada (`MEM_MAPPED`): archivos mapeados y secciones
    /// compartidas; los drivers de GPU suelen usarla.
    pub mapped: usize,
    /// Código y datos de ejecutables y DLL (`MEM_IMAGE`).
    pub image: usize,
    /// De `image`: `[programa, ffmpeg, drivers de GPU y DirectX, Windows y otras]`.
    pub groups: [usize; 4],
    /// Las DLL que más RAM ocupan (nombre, bytes).
    pub top: [(&'static str, usize); 6],
}

impl NativeBreakdown {
    pub fn is_empty(&self) -> bool {
        self.private + self.mapped + self.image == 0
    }
}

/// Nombres de DLL internados: son pocos y se repiten en cada medición, así
/// que se guardan una vez y se pasan como `&'static str` (el informe es `Copy`).
#[cfg(all(windows, target_pointer_width = "64"))]
fn intern(name: &str) -> &'static str {
    static NAMES: Mutex<Option<HashMap<String, &'static str>>> = Mutex::new(None);
    let mut guard = NAMES.lock().unwrap_or_else(|e| e.into_inner());
    let map = guard.get_or_insert_with(HashMap::new);
    if let Some(s) = map.get(name) {
        return s;
    }
    let leaked: &'static str = Box::leak(name.to_string().into_boxed_str());
    map.insert(name.to_string(), leaked);
    leaked
}

#[cfg(all(windows, target_pointer_width = "64"))]
pub fn native_breakdown() -> Option<NativeBreakdown> {
    use std::mem::{size_of, zeroed};

    #[repr(C)]
    struct MemInfo {
        base_address: usize,
        allocation_base: usize,
        allocation_protect: u32,
        partition_id: u16,
        region_size: usize,
        state: u32,
        protect: u32,
        kind: u32,
    }
    #[repr(C)]
    struct ModInfo {
        base: usize,
        size: u32,
        entry: usize,
    }

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetCurrentProcess() -> isize;
        fn VirtualQuery(address: usize, info: *mut MemInfo, len: usize) -> usize;
        fn K32QueryWorkingSet(process: isize, info: *mut usize, cb: u32) -> i32;
        fn K32EnumProcessModules(process: isize, modules: *mut isize, cb: u32, needed: *mut u32) -> i32;
        fn K32GetModuleInformation(process: isize, module: isize, info: *mut ModInfo, cb: u32) -> i32;
        fn K32GetModuleBaseNameW(process: isize, module: isize, name: *mut u16, size: u32) -> u32;
    }

    const MEM_COMMIT: u32 = 0x1000;
    const MEM_PRIVATE: u32 = 0x2_0000;
    const MEM_MAPPED: u32 = 0x4_0000;
    const MEM_IMAGE: u32 = 0x100_0000;
    const PAGE: usize = 4096;

    let process = unsafe { GetCurrentProcess() };

    // 1. Regiones comprometidas: (inicio, fin, tipo), ya ordenadas.
    let mut regions: Vec<(usize, usize, u32)> = Vec::new();
    let mut addr = 0usize;
    loop {
        let mut info: MemInfo = unsafe { zeroed() };
        let got = unsafe { VirtualQuery(addr, &mut info, size_of::<MemInfo>()) };
        if got == 0 {
            break;
        }
        let end = info.base_address.saturating_add(info.region_size);
        if info.state == MEM_COMMIT {
            regions.push((info.base_address, end, info.kind));
        }
        if end <= addr {
            break;
        }
        addr = end;
    }

    // 2. Módulos cargados (el primero es el ejecutable): (base, fin, nombre).
    let mut handles = vec![0isize; 1024];
    let mut needed = 0u32;
    let ok = unsafe {
        K32EnumProcessModules(process, handles.as_mut_ptr(), (handles.len() * size_of::<isize>()) as u32, &mut needed)
    };
    if ok == 0 {
        return None;
    }
    let count = (needed as usize / size_of::<isize>()).min(handles.len());
    let mut modules: Vec<(usize, usize, String)> = Vec::with_capacity(count);
    for &h in &handles[..count] {
        let mut mi = ModInfo { base: 0, size: 0, entry: 0 };
        if unsafe { K32GetModuleInformation(process, h, &mut mi, size_of::<ModInfo>() as u32) } == 0 {
            continue;
        }
        let mut name = [0u16; 260];
        let len = unsafe { K32GetModuleBaseNameW(process, h, name.as_mut_ptr(), name.len() as u32) } as usize;
        let name = String::from_utf16_lossy(&name[..len.min(name.len())]);
        modules.push((mi.base, mi.base + mi.size as usize, name));
    }
    let exe_name = std::env::current_exe()
        .ok()
        .and_then(|p| p.file_name().map(|n| n.to_string_lossy().to_ascii_lowercase()))
        .unwrap_or_default();
    modules.sort_by_key(|m| m.0);

    // 3. Páginas residentes. El primer `usize` es la cantidad de entradas y
    //    cada entrada trae el número de página en los bits altos.
    let mut buf: Vec<usize> = vec![0; (1 << 18) + 1];
    let mut tries = 0;
    loop {
        let ok = unsafe { K32QueryWorkingSet(process, buf.as_mut_ptr(), (buf.len() * size_of::<usize>()) as u32) };
        if ok != 0 {
            break;
        }
        let needed = buf[0];
        if tries >= 4 || needed == 0 {
            return None;
        }
        buf.resize(needed + 16_384 + 1, 0);
        tries += 1;
    }
    let entries = buf[0].min(buf.len() - 1);

    let mut out = NativeBreakdown::default();
    let mut per_module: Vec<usize> = vec![0; modules.len()];
    for &entry in &buf[1..=entries] {
        let page_addr = (entry >> 12) << 12;
        let idx = regions.partition_point(|r| r.1 <= page_addr);
        let Some(&(start, _, kind)) = regions.get(idx) else { continue };
        if start > page_addr {
            continue;
        }
        match kind {
            MEM_PRIVATE => out.private += PAGE,
            MEM_MAPPED => out.mapped += PAGE,
            MEM_IMAGE => {
                out.image += PAGE;
                let m = modules.partition_point(|m| m.1 <= page_addr);
                match modules.get(m) {
                    Some(module) if module.0 <= page_addr => per_module[m] += PAGE,
                    _ => out.groups[3] += PAGE,
                }
            }
            _ => {}
        }
    }

    // 4. Agrupar las DLL.
    let mut ranked: Vec<(usize, &str)> = Vec::new();
    for (i, module) in modules.iter().enumerate() {
        let bytes = per_module[i];
        if bytes == 0 {
            continue;
        }
        let lower = module.2.to_ascii_lowercase();
        let group = if lower == exe_name {
            0
        } else if ["avcodec", "avformat", "avutil", "swscale", "swresample", "avfilter", "avdevice", "postproc"]
            .iter()
            .any(|p| lower.starts_with(p))
        {
            1
        } else if [
            "nvoglv", "nvwgf", "nvd3d", "nvldumd", "nvapi", "nvcuda", "nvml", "nvdxgd", "igd", "ig9", "ig75", "igc",
            "intel", "amdvlk", "atiu", "aticfx", "atig", "amdxx", "amdxc", "amdihk", "amd_ags", "d3d", "dxgi",
            "dxcore", "dxil", "opengl32", "vulkan", "vk_swiftshader", "libegl", "libglesv2", "dcomp", "d2d1", "dwrite",
        ]
        .iter()
        .any(|p| lower.contains(p))
        {
            2
        } else {
            3
        };
        out.groups[group] += bytes;
        ranked.push((bytes, module.2.as_str()));
    }
    ranked.sort_by(|a, b| b.0.cmp(&a.0));
    for (slot, (bytes, name)) in out.top.iter_mut().zip(ranked) {
        *slot = (intern(name), bytes);
    }
    Some(out)
}

#[cfg(not(all(windows, target_pointer_width = "64")))]
pub fn native_breakdown() -> Option<NativeBreakdown> {
    None
}

// ---------------------------------------------------------------------
// Formato
// ---------------------------------------------------------------------

/// `30 MB`, `1.2 MB`, `850 KB`, `1.25 GB`.
pub fn fmt_bytes(bytes: usize) -> String {
    fmt_bytes_u64(bytes as u64)
}

pub fn fmt_bytes_u64(bytes: u64) -> String {
    const KB: f64 = 1024.0;
    const MB: f64 = KB * 1024.0;
    const GB: f64 = MB * 1024.0;
    let b = bytes as f64;
    if b >= GB {
        format!("{:.2} GB", b / GB)
    } else if b >= 10.0 * MB {
        format!("{:.0} MB", b / MB)
    } else if b >= MB {
        format!("{:.1} MB", b / MB)
    } else if b >= KB {
        format!("{:.0} KB", b / KB)
    } else {
        format!("{bytes} B")
    }
}

// ---------------------------------------------------------------------
// Estimación del tamaño en heap de las estructuras de datos
// ---------------------------------------------------------------------

/// Bytes que una estructura tiene APARTE de su propio `size_of` (strings,
/// vectores, mapas...). Los contenedores (`Vec`, `Option`, `HashMap`...) suman
/// además el espacio de los elementos que contienen.
pub trait HeapBytes {
    fn heap_bytes(&self) -> usize;
}

macro_rules! no_heap {
    ($($t:ty),* $(,)?) => {
        $(impl HeapBytes for $t {
            fn heap_bytes(&self) -> usize { 0 }
        })*
    };
}

no_heap!(
    bool, u8, u16, u32, u64, usize, i8, i16, i32, i64, isize, f32, f64, char,
    egui::Color32, std::time::Instant, crate::lib::data::Status,
);

impl HeapBytes for String {
    fn heap_bytes(&self) -> usize {
        self.capacity()
    }
}

impl<T: HeapBytes> HeapBytes for Option<T> {
    fn heap_bytes(&self) -> usize {
        self.as_ref().map_or(0, HeapBytes::heap_bytes)
    }
}

impl<T: HeapBytes> HeapBytes for Box<T> {
    fn heap_bytes(&self) -> usize {
        size_of::<T>() + (**self).heap_bytes()
    }
}

impl<T: HeapBytes> HeapBytes for Vec<T> {
    fn heap_bytes(&self) -> usize {
        self.capacity() * size_of::<T>() + self.iter().map(HeapBytes::heap_bytes).sum::<usize>()
    }
}

impl<T: HeapBytes> HeapBytes for VecDeque<T> {
    fn heap_bytes(&self) -> usize {
        self.capacity() * size_of::<T>() + self.iter().map(HeapBytes::heap_bytes).sum::<usize>()
    }
}

impl<A: HeapBytes, B: HeapBytes> HeapBytes for (A, B) {
    fn heap_bytes(&self) -> usize {
        self.0.heap_bytes() + self.1.heap_bytes()
    }
}

impl<K: HeapBytes, V: HeapBytes, S> HeapBytes for HashMap<K, V, S> {
    fn heap_bytes(&self) -> usize {
        // hashbrown: un byte de control por casilla, más clave y valor.
        self.capacity() * (size_of::<K>() + size_of::<V>() + 1)
            + self.iter().map(|(k, v)| k.heap_bytes() + v.heap_bytes()).sum::<usize>()
    }
}

impl<K: HeapBytes, S> HeapBytes for HashSet<K, S> {
    fn heap_bytes(&self) -> usize {
        self.capacity() * (size_of::<K>() + 1) + self.iter().map(HeapBytes::heap_bytes).sum::<usize>()
    }
}

impl HeapBytes for serde_json::Value {
    fn heap_bytes(&self) -> usize {
        use serde_json::Value;
        match self {
            Value::Null | Value::Bool(_) | Value::Number(_) => 0,
            Value::String(s) => s.capacity(),
            Value::Array(items) => {
                items.capacity() * size_of::<Value>()
                    + items.iter().map(HeapBytes::heap_bytes).sum::<usize>()
            }
            // ~48 bytes de nodo por entrada del mapa, más clave y valor.
            Value::Object(map) => map
                .iter()
                .map(|(k, v)| 48 + k.capacity() + size_of::<Value>() + v.heap_bytes())
                .sum(),
        }
    }
}

/// `impl HeapBytes` para un struct sumando solo los campos que tienen heap
/// (los numéricos, bools y colores no aportan). Las listas de campos están
/// generadas a partir de las definiciones reales de cada struct: si se agrega
/// un campo con `String`/`Vec`/`Option<String>`... conviene sumarlo acá.
macro_rules! heap_struct {
    ($ty:ty { $($field:ident),* $(,)? }) => {
        impl HeapBytes for $ty {
            fn heap_bytes(&self) -> usize {
                0 $(+ self.$field.heap_bytes())*
            }
        }
    };
}

impl HeapBytes for crate::lib::data::ReactionKind {
    fn heap_bytes(&self) -> usize {
        match self {
            Self::Unicode(s) => s.heap_bytes(),
            Self::Custom { id, name, .. } => id.heap_bytes() + name.heap_bytes(),
        }
    }
}

heap_struct!(crate::discord::models::User { id, username, global_name, discriminator, avatar, banner, bio, pronouns, avatar_decoration_data, banner_color, primary_guild, clan, display_name_styles });
heap_struct!(crate::discord::models::Attachment { id, filename, content_type, url, proxy_url });
heap_struct!(crate::discord::models::StickerItem { id, name });
heap_struct!(crate::discord::models::EmbedMedia { url, proxy_url });
heap_struct!(crate::discord::models::EmbedFooter { text, icon_url, proxy_icon_url });
heap_struct!(crate::discord::models::EmbedAuthor { name, url, icon_url, proxy_icon_url });
heap_struct!(crate::discord::models::EmbedProvider { name, url });
heap_struct!(crate::discord::models::EmbedField { name, value });
heap_struct!(crate::discord::models::Embed { kind, title, description, url, timestamp, footer, image, thumbnail, video, provider, author, fields });
heap_struct!(crate::discord::models::ComponentEmoji { id, name });
heap_struct!(crate::discord::models::GalleryItem { media, description });
heap_struct!(crate::discord::models::Component { id, custom_id, label, emoji, url, placeholder, content, components, accessory, component, description, value, items, media, file, name });
heap_struct!(crate::discord::models::Role { id, name });
heap_struct!(crate::discord::models::PermissionOverwrite { id });
heap_struct!(crate::discord::models::ForumTag { id, name, emoji_name });
heap_struct!(crate::discord::models::PartialMember { nick, user, roles });
heap_struct!(crate::discord::models::Presence { status, activities });
heap_struct!(crate::discord::models::PresenceActivity { name, state, details, application_id, timestamps, assets });
heap_struct!(crate::discord::models::MemberListGroup { id });
heap_struct!(crate::discord::models::MemberListMember { user, nick, roles, presence });
heap_struct!(crate::discord::models::MemberListItem { group, member });
heap_struct!(crate::discord::models::Channel { id, name, parent_id, permission_overwrites, available_tags });
heap_struct!(crate::discord::models::PrivateChannel { safety_warnings, recipients, last_message_id, is_message_request_timestamp, id, username, avatar, avatar_url });
heap_struct!(crate::discord::models::VoiceState { guild_id, channel_id, user_id, session_id, member });
heap_struct!(crate::lib::data::Reaction { emoji });
heap_struct!(crate::lib::data::ReplyTarget { message_id, author, preview });
heap_struct!(crate::lib::data::RepliedMessage { author, author_id, author_base, preview });
heap_struct!(crate::lib::data::ForwardedMessage { content, time, mentions, attachments, embeds, stickers });
heap_struct!(crate::lib::data::Forward { snapshots, source_channel_id, source_guild_id });
heap_struct!(crate::lib::data::ChatMessage { id, author, author_id, author_base, time, content, avatar_url, mentions, reactions, attachments, embeds, stickers, replied_to, forwarded, components, thread, channel_id, application_id, interaction });
heap_struct!(crate::lib::data::InteractionLine { user_id, user, avatar_url, command });
heap_struct!(crate::lib::data::ThreadCard { id, name, last_message_id, owner_id });
heap_struct!(crate::lib::data::Friend { name, subtitle, avatar_url, user_id, dm_channel_id, handle, member_since, messages });
heap_struct!(crate::lib::data::VoiceOccupant { user_id, name, avatar_url });
heap_struct!(crate::lib::data::Channel { name, messages, channel_id, voice_members, overwrites, parent_id, forum });
heap_struct!(crate::lib::data::ForumState { tags, posts, search, draft_title, draft_body, draft_tags });
heap_struct!(crate::lib::data::ForumPost { id, title, author, preview, thumbnail, tag_ids, reaction });
heap_struct!(crate::lib::data::ChannelCategory { name, channels });
heap_struct!(crate::lib::data::Member { name, avatar_url, user_id, subtitle });
heap_struct!(crate::lib::data::MemberGroup { name, members });
heap_struct!(crate::lib::data::Server { name, icon_initial, icon_url, banner_url, guild_id, topic, categories, member_groups, roles, member_lists, active_member_list, channel_member_list, pending_list_channel, custom_emojis, custom_stickers, member_info, requested_members, pending_voice_states, known_users, access_ctx, raw_channels });
heap_struct!(crate::lib::data::MemberInfo { nick, roles });
heap_struct!(crate::lib::data::KnownUser { name, avatar_url });
heap_struct!(crate::lib::data::MemberListState { items });
heap_struct!(crate::lib::data::CustomEmoji { id, name });
heap_struct!(crate::lib::data::ActivityCard { name, detail, time, connect_label });
heap_struct!(crate::lib::permissions::AccessContext { owner_id, my_id, my_roles });
