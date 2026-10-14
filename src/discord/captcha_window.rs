//! Ventana del captcha (proceso hijo): WebView con el widget de hCaptcha.
//!
//! Este archivo es lo único que usa `wry`/`tao` y, en Linux, lo único que
//! enlaza WebKitGTK/GTK. Por eso se compila en dos lugares:
//!
//! * Windows/macOS: dentro de `ecord` (se lanza con `--captcha-window`).
//! * Linux: dentro del ejecutable aparte `ecord-captcha`
//!   (`src/bin/ecord-captcha.rs`), para que `ecord` arranque aunque el
//!   sistema no tenga WebKitGTK instalado.
//!
//! Se incluye con `#[path]` y solo depende de `super::proto` (constantes).

use super::proto::{ENV_RQDATA, ENV_SITEKEY, ENV_USER_AGENT, ERROR_PREFIX, HOST_PAGE, TOKEN_PREFIX};

/// Cuerpo del proceso hijo: abre la ventana, espera el token, lo imprime
/// por stdout y termina. Nunca vuelve.
pub fn run_window() -> ! {
    use std::io::Write;
    use tao::dpi::LogicalSize;
    use tao::event::{Event, WindowEvent};
    use tao::event_loop::{ControlFlow, EventLoopBuilder};
    use tao::window::WindowBuilder;
    use wry::WebViewBuilder;

    // Cualquier fallo de arranque (WebView2 ausente, carpeta de datos sin
    // permisos, panic) se le avisa al proceso padre por stdout: en release
    // este proceso no tiene consola y antes la ventana se cerraba sin dejar
    // ningún rastro.
    std::panic::set_hook(Box::new(|info| {
        report_fatal(&format!("panic: {info}"));
    }));
    #[cfg(windows)]
    if std::env::var_os("WEBVIEW2_USER_DATA_FOLDER").is_none() {
        if let Some(dir) = crate::paths::webview_data_dir() {
            let _ = std::fs::create_dir_all(&dir);
            // SAFETY: todavía es el único hilo del proceso hijo.
            unsafe { std::env::set_var("WEBVIEW2_USER_DATA_FOLDER", dir) };
        }
    }

    let sitekey = std::env::var(ENV_SITEKEY).unwrap_or_default();
    let rqdata = std::env::var(ENV_RQDATA).ok();
    let user_agent = std::env::var(ENV_USER_AGENT).ok();

    let event_loop = EventLoopBuilder::<String>::with_user_event().build();
    let proxy = event_loop.create_proxy();
    let window = WindowBuilder::new()
        .with_title("eCord — Verificación")
        .with_inner_size(LogicalSize::new(500.0, 680.0))
        .build(&event_loop)
        .unwrap_or_else(|error| report_fatal(&format!("no se pudo crear la ventana: {error}")));

    let script = init_script(&sitekey, rqdata.as_deref());
    let mut builder = WebViewBuilder::new()
        .with_url(HOST_PAGE)
        .with_initialization_script(&script)
        .with_ipc_handler(move |request: wry::http::Request<String>| {
            let _ = proxy.send_event(request.body().clone());
        });
    if let Some(user_agent) = &user_agent {
        builder = builder.with_user_agent(user_agent);
    }
    let webview = builder
        .build(&window)
        .unwrap_or_else(|error| {
            report_fatal(&format!(
                "no se pudo crear el WebView ({error}); ¿falta WebView2 (Windows) o webkit2gtk (Linux)?"
            ))
        });

    event_loop.run(move |event, _, control_flow| {
        *control_flow = ControlFlow::Wait;
        // El WebView tiene que vivir mientras dure el event loop.
        let _keep_alive = &webview;
        match event {
            Event::UserEvent(token) => {
                println!("{TOKEN_PREFIX}{token}");
                let _ = std::io::stdout().flush();
                *control_flow = ControlFlow::Exit;
            }
            Event::WindowEvent {
                event: WindowEvent::CloseRequested,
                ..
            } => *control_flow = ControlFlow::Exit,
            _ => {}
        }
    })
}

/// Avisa del error al proceso padre (stdout) y a stderr, y termina.
fn report_fatal(message: &str) -> ! {
    use std::io::Write;
    let one_line = message.replace(['\r', '\n'], " ");
    println!("{ERROR_PREFIX}{one_line}");
    let _ = std::io::stdout().flush();
    eprintln!("[captcha-window] {one_line}");
    std::process::exit(1)
}

/// Script que corre al crear cada documento: en la página de discord.com
/// (solo frame principal) frena la carga de la app, arma una página mínima
/// con el widget de hCaptcha y manda el token por `window.ipc`.
fn init_script(sitekey: &str, rqdata: Option<&str>) -> String {
    let sitekey = serde_json::to_string(sitekey).unwrap_or_else(|_| "\"\"".to_owned());
    let rqdata = match rqdata {
        Some(data) => serde_json::to_string(data).unwrap_or_else(|_| "null".to_owned()),
        None => "null".to_owned(),
    };
    format!(
        r#"(function () {{
  if (window.top !== window || location.hostname !== 'discord.com') return;
  var SITEKEY = {sitekey};
  var RQDATA = {rqdata};

  function boot() {{
    if (window.__ecordBooted) return;
    window.__ecordBooted = true;
    try {{ window.stop(); }} catch (e) {{}}
    document.open();
    document.write('<!doctype html><html><head><meta charset="utf-8"><title>eCord</title></head><body></body></html>');
    document.close();

    var style = document.createElement('style');
    style.textContent = 'html,body{{margin:0;height:100%;background:#1e1f22;color:#dbdee1;font-family:system-ui,Segoe UI,sans-serif}}' +
      'body{{display:flex;flex-direction:column;align-items:center;justify-content:center;gap:16px;text-align:center}}' +
      'p{{margin:0;padding:0 24px;font-size:14px}}';
    document.head.appendChild(style);

    var msg = document.createElement('p');
    msg.textContent = 'Necesitamos verificar que eres un humano.';
    document.body.appendChild(msg);
    var box = document.createElement('div');
    box.id = 'ecord-hcaptcha';
    document.body.appendChild(box);

    var widgetId = null;
    window.ecordCaptchaReady = function () {{
      try {{
        widgetId = hcaptcha.render('ecord-hcaptcha', {{
          sitekey: SITEKEY,
          theme: 'dark',
          callback: function (token) {{
            msg.textContent = 'Listo, volviendo a eCord…';
            window.ipc.postMessage(token);
          }},
          'expired-callback': function () {{ try {{ hcaptcha.reset(widgetId); }} catch (e) {{}} }},
          'error-callback': function (err) {{ msg.textContent = 'Error del captcha: ' + err; }}
        }});
        if (RQDATA) {{ hcaptcha.setData(widgetId, {{ rqdata: RQDATA }}); }}
      }} catch (e) {{
        msg.textContent = 'No se pudo mostrar el captcha: ' + e;
      }}
    }};

    var script = document.createElement('script');
    script.src = 'https://js.hcaptcha.com/1/api.js?render=explicit&onload=ecordCaptchaReady';
    script.async = true;
    script.onerror = function () {{ msg.textContent = 'No se pudo cargar hCaptcha (revisá tu conexión).'; }};
    document.head.appendChild(script);

    setTimeout(function () {{
      if (typeof hcaptcha === 'undefined') {{
        msg.textContent = 'hCaptcha no cargó. Cerrá esta ventana y probá de nuevo o usá el QR.';
      }}
    }}, 15000);
  }}

  if (document.readyState === 'loading') {{
    document.addEventListener('DOMContentLoaded', boot);
  }} else {{
    boot();
  }}
}})();"#
    )
}

