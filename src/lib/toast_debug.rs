//! Diagnóstico de las notificaciones de escritorio (comando `toast` de la
//! consola de las devtools). En Windows manda un toast directo con WinRT,
//! paso a paso, y escribe en el registro el HRESULT REAL de cada fallo (la
//! librería `fastframe-notifications` lo convierte todo en un genérico
//! "delivery failed").

#[cfg(windows)]
pub fn run(aumid: &str, title: &str, body: &str) {
    let (aumid, title, body) = (aumid.to_owned(), title.to_owned(), body.to_owned());
    // Hilo aparte: espera unos segundos los eventos del toast sin trabar la UI.
    let _ = std::thread::Builder::new()
        .name("ecord-toast-debug".into())
        .spawn(move || {
            log::info!(target: "toast", "── prueba de toast con AUMID «{aumid}» ──");
            match steps(&aumid, &title, &body) {
                Ok(()) => log::info!(target: "toast", "fin de la prueba"),
                Err(e) => log::error!(
                    target: "toast",
                    "la prueba terminó con error: {e:?} (HRESULT {:#010x})",
                    e.code().0 as u32
                ),
            }
        });
}

#[cfg(not(windows))]
pub fn run(_aumid: &str, _title: &str, _body: &str) {
    log::warn!(target: "toast", "esta prueba directa es solo para Windows; usá `toast ff`");
}

/// Registra el resultado de un paso (con su HRESULT si falló) y lo devuelve.
#[cfg(windows)]
fn step<T>(name: &str, result: windows::core::Result<T>) -> windows::core::Result<T> {
    match &result {
        Ok(_) => log::info!(target: "toast", "{name}: ok"),
        Err(e) => log::error!(
            target: "toast",
            "{name} FALLÓ: {e:?} (HRESULT {:#010x})",
            e.code().0 as u32
        ),
    }
    result
}

#[cfg(windows)]
fn steps(aumid: &str, title: &str, body: &str) -> windows::core::Result<()> {
    use windows::{
        Data::Xml::Dom::XmlDocument,
        Foundation::TypedEventHandler,
        UI::Notifications::{
            ToastDismissedEventArgs, ToastFailedEventArgs, ToastNotification, ToastNotificationManager,
        },
        core::{HSTRING, IInspectable, Ref},
    };

    // ¿Existe el acceso directo del menú Inicio con el AUMID?
    if let Some(appdata) = std::env::var_os("APPDATA") {
        let lnk = std::path::PathBuf::from(appdata)
            .join(r"Microsoft\Windows\Start Menu\Programs")
            .join("eCord.lnk");
        log::info!(target: "toast", "acceso directo {}: existe={}", lnk.display(), lnk.exists());
    }

    if aumid == crate::lib::win_toast::AUMID {
        step("register_identity", crate::lib::win_toast::register_identity(aumid, "eCord"))?;
    }
    let notifier = step(
        "CreateToastNotifierWithId",
        ToastNotificationManager::CreateToastNotifierWithId(&HSTRING::from(aumid)),
    )?;
    // Un error acá NO corta la prueba: lo importante es si `Show` funciona.
    match step("Setting", notifier.Setting()) {
        Ok(setting) => log::info!(target: "toast", "Setting = {setting:?} (Enabled = permitidas)"),
        Err(_) => log::info!(target: "toast", "se sigue igual con Show…"),
    }

    let esc = |s: &str| s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;");
    let xml = format!(
        "<toast><visual><binding template=\"ToastGeneric\"><text>{}</text><text>{}</text></binding></visual></toast>",
        esc(title),
        esc(body)
    );
    let doc = step("XmlDocument::new", XmlDocument::new())?;
    step("LoadXml", doc.LoadXml(&HSTRING::from(xml)))?;
    let toast = step("CreateToastNotification", ToastNotification::CreateToastNotification(&doc))?;

    step(
        "Activated",
        toast.Activated(&TypedEventHandler::new(|_, _: Ref<'_, IInspectable>| {
            log::info!(target: "toast", "evento: Activated (clic en el toast)");
            Ok(())
        })),
    )?;
    step(
        "Dismissed",
        toast.Dismissed(&TypedEventHandler::new(|_, args: Ref<'_, ToastDismissedEventArgs>| {
            let reason = args.as_ref().and_then(|a| a.Reason().ok());
            log::info!(target: "toast", "evento: Dismissed ({reason:?})");
            Ok(())
        })),
    )?;
    step(
        "Failed",
        toast.Failed(&TypedEventHandler::new(|_, args: Ref<'_, ToastFailedEventArgs>| {
            match args.as_ref().map(|a| a.ErrorCode()) {
                Some(Ok(code)) => log::error!(
                    target: "toast",
                    "evento: Failed — HRESULT {:#010x}",
                    code.0 as u32
                ),
                other => log::error!(target: "toast", "evento: Failed — sin código ({other:?})"),
            }
            Ok(())
        })),
    )?;

    step("Show", notifier.Show(&toast))?;
    log::info!(target: "toast", "Show aceptado; esperando eventos 8 s (si no aparece ni hay Failed, mirá Foco/No molestar)…");
    std::thread::sleep(std::time::Duration::from_secs(8));
    Ok(())
}
