//! Windows: identidad de la app para las notificaciones de escritorio.
//!
//! Una app sin instalar (por ejemplo `cargo run`) no tiene acceso directo en
//! el menú Inicio con un AppUserModelID, y Windows puede rechazar sus toasts.
//! Acá se (1) fija el AUMID del proceso y (2) crea/actualiza, en un hilo
//! aparte, `%APPDATA%\Microsoft\Windows\Start Menu\Programs\eCord.lnk`
//! apuntando al .exe en ejecución y con ese mismo AUMID.

#![cfg(windows)]

use std::path::PathBuf;

use windows::{
    core::{Interface, GUID, HSTRING, PCWSTR, PWSTR},
    Win32::{
        Foundation::PROPERTYKEY,
        System::{
            Com::{
                CoCreateInstance, CoInitializeEx, CoUninitialize, IPersistFile,
                StructuredStorage::PROPVARIANT, CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED,
            },
            Variant::VT_LPWSTR,
        },
        UI::Shell::{
            PropertiesSystem::IPropertyStore, IShellLinkW, SetCurrentProcessExplicitAppUserModelID,
            ShellLink,
        },
    },
};

/// Tiene que ser IGUAL al id de `fastframe_notifications::App::new(..)`.
pub const AUMID: &str = "dev.ecord.client";

/// PKEY_AppUserModel_ID {9F4C2855-9F79-4B39-A8D0-E1D42DE1D5F3}, 5
const PKEY_APP_USER_MODEL_ID: PROPERTYKEY = PROPERTYKEY {
    fmtid: GUID::from_u128(0x9F4C2855_9F79_4B39_A8D0_E1D42DE1D5F3),
    pid: 5,
};

/// Llamar al principio de `main` (después de `logging::init()`).
pub fn init() {
    // El proceso y el acceso directo deben compartir AUMID.
    if let Err(err) = unsafe { SetCurrentProcessExplicitAppUserModelID(&HSTRING::from(AUMID)) } {
        log::warn!("no se pudo fijar el AUMID del proceso: {err:?}");
    }
    // COM de apartamento simple (STA) en un hilo propio, sin tocar el de eframe.
    let _ = std::thread::Builder::new()
        .name("ecord-start-menu-shortcut".into())
        .spawn(|| match ensure_shortcut() {
            Ok(path) => log::info!("acceso directo de notificaciones listo: {}", path.display()),
            Err(err) => log::warn!("no se pudo crear el acceso directo de eCord: {err:?}"),
        });
}

fn shortcut_path() -> Option<PathBuf> {
    let appdata = std::env::var_os("APPDATA")?;
    Some(
        PathBuf::from(appdata)
            .join(r"Microsoft\Windows\Start Menu\Programs")
            .join("eCord.lnk"),
    )
}

fn ensure_shortcut() -> windows::core::Result<PathBuf> {
    let lnk = shortcut_path().ok_or_else(|| windows::core::Error::from(windows::Win32::Foundation::E_FAIL))?;
    let exe = std::env::current_exe()
        .map_err(|_| windows::core::Error::from(windows::Win32::Foundation::E_FAIL))?;

    unsafe {
        // S_FALSE (ya inicializado) también sirve; solo importa no fallar fuerte.
        let init = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        let result = (|| -> windows::core::Result<()> {
            let link: IShellLinkW = CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER)?;
            link.SetPath(&HSTRING::from(exe.as_os_str()))?;
            if let Some(dir) = exe.parent() {
                link.SetWorkingDirectory(&HSTRING::from(dir.as_os_str()))?;
            }
            link.SetDescription(&HSTRING::from("eCord"))?;
            link.SetIconLocation(&HSTRING::from(exe.as_os_str()), 0)?;

            let store: IPropertyStore = link.cast()?;
            // PROPVARIANT de tipo VT_LPWSTR armado a mano. `ManuallyDrop` evita que
            // se intente liberar el buffer (es memoria de Rust, no de COM); el
            // almacén copia el valor en `SetValue`.
            let mut text: Vec<u16> = AUMID.encode_utf16().chain(Some(0)).collect();
            let mut pv: std::mem::ManuallyDrop<PROPVARIANT> =
                std::mem::ManuallyDrop::new(std::mem::zeroed());
            {
                let inner = &mut *pv.Anonymous.Anonymous;
                inner.vt = VT_LPWSTR;
                inner.Anonymous.pwszVal = PWSTR(text.as_mut_ptr());
            }
            store.SetValue(&PKEY_APP_USER_MODEL_ID, &*pv)?;
            store.Commit()?;

            let file: IPersistFile = link.cast()?;
            file.Save(PCWSTR(HSTRING::from(lnk.as_os_str()).as_ptr()), true)?;
            Ok(())
        })();
        if init.is_ok() {
            CoUninitialize();
        }
        result?;
    }
    Ok(lnk)
}
