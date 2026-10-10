//! Ventanas que se pueden transmitir con "Compartir pantalla".
//!
//! El selector de `ui::share_picker` las muestra y `CaptureTarget::Window`
//! (`screen_capture.rs`) captura por `HWND` con Windows Graphics Capture.
//! Hoy solo hay lista en Windows; en el resto de sistemas queda vacía y el
//! selector ofrece únicamente la pantalla completa.
//!
//! La lista se arma como la de Alt+Tab: ventanas visibles de nivel superior,
//! con título, sin ventanas de herramientas, sin las "fantasma" de las apps UWP
//! suspendidas (cloaked) y sin las del propio eCord (transmitirse a sí mismo
//! solo arma un espejo infinito). Viene en orden de Z (la de arriba primero),
//! que en la práctica es "la que usaste hace menos".

use std::collections::HashSet;

/// Una ventana de escritorio candidata a transmitirse.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct CaptureWindow {
    /// Título de la ventana: lo que se muestra en el selector (y el respaldo
    /// `gdigrab title=...` si WGC no está disponible).
    pub(crate) title: String,
    /// `HWND` de la ventana: con esto la encuentra Windows Graphics Capture,
    /// sin importar que cambie de título o que haya otra con el mismo.
    pub(crate) hwnd: isize,
    /// Nombre del ejecutable sin `.exe` (`chrome`, `Code`), si se pudo leer.
    pub(crate) app: Option<String>,
    /// Minimizada: no se puede capturar hasta que se restaure.
    pub(crate) minimized: bool,
}

/// Una pantalla (monitor) que se puede transmitir entera.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct CaptureMonitor {
    /// "Pantalla 1", "Pantalla 2"... (la principal siempre es la 1).
    pub(crate) label: String,
    /// Esquina superior izquierda en el escritorio virtual.
    pub(crate) x: i32,
    pub(crate) y: i32,
    pub(crate) width: u32,
    pub(crate) height: u32,
    pub(crate) primary: bool,
}

/// Pantallas conectadas, la principal primero. Vacía donde no hay captura por
/// pantalla puntual (allá se ofrece solo el escritorio completo).
pub(crate) fn list_capture_monitors() -> Vec<CaptureMonitor> {
    let mut monitors = imp::list_monitors();
    monitors.sort_by_key(|monitor| (!monitor.primary, monitor.x, monitor.y));
    for (index, monitor) in monitors.iter_mut().enumerate() {
        monitor.label = format!("Pantalla {}", index + 1);
    }
    monitors
}

/// ¿Esta plataforma puede transmitir una ventana suelta?
pub(crate) const WINDOW_CAPTURE_SUPPORTED: bool = cfg!(target_os = "windows");

/// Ventanas abiertas ahora mismo que se pueden elegir. Vacía donde no hay
/// captura por ventana (ver `WINDOW_CAPTURE_SUPPORTED`).
pub(crate) fn list_capture_windows() -> Vec<CaptureWindow> {
    let mut windows = imp::list();
    windows.retain(|window| !is_shell_noise(&window.title));
    dedupe_by_title(windows)
}

/// Ventanas del propio sistema que están "visibles" pero no son una app.
fn is_shell_noise(title: &str) -> bool {
    matches!(title, "Program Manager" | "Windows Input Experience")
}

/// Dos ventanas con el mismo título no se pueden distinguir en el selector (y
/// el respaldo `gdigrab` se queda con la primera que coincide): se deja solo
/// la primera, la de más arriba.
fn dedupe_by_title(windows: Vec<CaptureWindow>) -> Vec<CaptureWindow> {
    let mut seen = HashSet::new();
    windows
        .into_iter()
        .filter(|window| seen.insert(window.title.clone()))
        .collect()
}

#[cfg(windows)]
mod imp {
    use std::ffi::c_void;
    use std::path::Path;

    use windows_sys::Win32::Foundation::{CloseHandle, HWND, LPARAM, RECT};
    use windows_sys::Win32::Graphics::Gdi::{
        EnumDisplayMonitors, GetMonitorInfoW, HDC, HMONITOR, MONITORINFO,
    };
    use windows_sys::Win32::Graphics::Dwm::{DWMWA_CLOAKED, DwmGetWindowAttribute};
    use windows_sys::Win32::System::Threading::{
        GetCurrentProcessId, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
        QueryFullProcessImageNameW,
    };
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        EnumWindows, GW_OWNER, GWL_EXSTYLE, GetWindow, GetWindowLongW, GetWindowTextLengthW,
        GetWindowTextW, GetWindowThreadProcessId, IsIconic, IsWindowVisible, WS_EX_APPWINDOW,
        WS_EX_TOOLWINDOW,
    };

    use super::{CaptureMonitor, CaptureWindow};

    /// `MONITORINFO::dwFlags`: es la pantalla principal (windows-sys no
    /// exporta esta constante).
    const MONITORINFOF_PRIMARY: u32 = 1;

    pub(super) fn list_monitors() -> Vec<CaptureMonitor> {
        let mut monitors: Vec<CaptureMonitor> = Vec::new();
        // SAFETY: `monitor_proc` solo castea `lparam` de vuelta al `Vec` de
        // arriba, que sigue vivo durante toda la llamada (síncrona).
        unsafe {
            EnumDisplayMonitors(
                std::ptr::null_mut(),
                std::ptr::null(),
                Some(monitor_proc),
                &mut monitors as *mut Vec<CaptureMonitor> as LPARAM,
            );
        }
        monitors
    }

    /// Devuelve 1 (TRUE) para seguir enumerando.
    extern "system" fn monitor_proc(monitor: HMONITOR, _dc: HDC, _rect: *mut RECT, lparam: LPARAM) -> i32 {
        // SAFETY: `lparam` es el `&mut Vec<CaptureMonitor>` que pasó `list_monitors`.
        let monitors = unsafe { &mut *(lparam as *mut Vec<CaptureMonitor>) };
        // SAFETY: `info` es un destino válido con `cbSize` puesto.
        let mut info: MONITORINFO = unsafe { std::mem::zeroed() };
        info.cbSize = std::mem::size_of::<MONITORINFO>() as u32;
        if unsafe { GetMonitorInfoW(monitor, &mut info) } != 0 {
            let rect = info.rcMonitor;
            let (width, height) = (rect.right - rect.left, rect.bottom - rect.top);
            if width > 0 && height > 0 {
                monitors.push(CaptureMonitor {
                    label: String::new(),
                    x: rect.left,
                    y: rect.top,
                    width: width as u32,
                    height: height as u32,
                    primary: info.dwFlags & MONITORINFOF_PRIMARY != 0,
                });
            }
        }
        1
    }

    pub(super) fn list() -> Vec<CaptureWindow> {
        let mut windows: Vec<CaptureWindow> = Vec::new();
        // SAFETY: `enum_proc` solo castea `lparam` de vuelta al `Vec` de arriba,
        // que sigue vivo durante toda la llamada (EnumWindows es síncrona).
        unsafe {
            EnumWindows(Some(enum_proc), &mut windows as *mut Vec<CaptureWindow> as LPARAM);
        }
        windows
    }

    /// Devuelve 1 (TRUE) para seguir enumerando.
    extern "system" fn enum_proc(hwnd: HWND, lparam: LPARAM) -> i32 {
        // SAFETY: `lparam` es el `&mut Vec<CaptureWindow>` que pasó `list`.
        let windows = unsafe { &mut *(lparam as *mut Vec<CaptureWindow>) };
        if let Some(window) = describe(hwnd) {
            windows.push(window);
        }
        1
    }

    fn describe(hwnd: HWND) -> Option<CaptureWindow> {
        // SAFETY: llamadas de solo lectura a user32/dwmapi con un HWND que
        // acaba de entregar EnumWindows y buffers propios del tamaño que se
        // declara.
        unsafe {
            if IsWindowVisible(hwnd) == 0 {
                return None;
            }
            let ex_style = GetWindowLongW(hwnd, GWL_EXSTYLE) as u32;
            if ex_style & WS_EX_TOOLWINDOW != 0 {
                return None;
            }
            // Ventanas con dueño (diálogos, paletas) no aparecen en Alt+Tab,
            // salvo que se declaren ventana de aplicación.
            if !GetWindow(hwnd, GW_OWNER).is_null() && ex_style & WS_EX_APPWINDOW == 0 {
                return None;
            }

            // Ventanas de apps UWP suspendidas: "visibles" pero ocultas por DWM.
            let mut cloaked: u32 = 0;
            let status = DwmGetWindowAttribute(
                hwnd,
                DWMWA_CLOAKED as _,
                &mut cloaked as *mut u32 as *mut c_void,
                std::mem::size_of::<u32>() as u32,
            );
            if status >= 0 && cloaked != 0 {
                return None;
            }

            let length = GetWindowTextLengthW(hwnd);
            if length <= 0 {
                return None;
            }
            let mut buffer = vec![0u16; length as usize + 1];
            let copied = GetWindowTextW(hwnd, buffer.as_mut_ptr(), buffer.len() as i32);
            if copied <= 0 {
                return None;
            }
            let title = String::from_utf16_lossy(&buffer[..copied as usize]);

            let mut pid: u32 = 0;
            GetWindowThreadProcessId(hwnd, &mut pid);
            if pid == GetCurrentProcessId() {
                return None;
            }

            Some(CaptureWindow {
                title,
                hwnd: hwnd as isize,
                app: process_name(pid),
                minimized: IsIconic(hwnd) != 0,
            })
        }
    }

    /// Nombre del ejecutable (sin ruta ni `.exe`) del proceso `pid`.
    fn process_name(pid: u32) -> Option<String> {
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
            Path::new(&path)
                .file_stem()
                .map(|stem| stem.to_string_lossy().into_owned())
        }
    }
}

#[cfg(not(windows))]
mod imp {
    use super::{CaptureMonitor, CaptureWindow};

    pub(super) fn list() -> Vec<CaptureWindow> {
        Vec::new()
    }

    pub(super) fn list_monitors() -> Vec<CaptureMonitor> {
        Vec::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn window(title: &str, app: &str) -> CaptureWindow {
        CaptureWindow {
            title: title.to_owned(),
            hwnd: 0,
            app: Some(app.to_owned()),
            minimized: false,
        }
    }

    #[test]
    fn duplicate_titles_keep_the_first_window() {
        let list = dedupe_by_title(vec![
            window("Notas", "notepad"),
            window("Chrome", "chrome"),
            window("Notas", "otra"),
        ]);
        assert_eq!(list.len(), 2);
        assert_eq!(list[0].app.as_deref(), Some("notepad"));
        assert_eq!(list[1].title, "Chrome");
    }

    #[test]
    fn desktop_shell_window_is_noise() {
        assert!(is_shell_noise("Program Manager"));
        assert!(!is_shell_noise("Mi documento - Word"));
    }
}
