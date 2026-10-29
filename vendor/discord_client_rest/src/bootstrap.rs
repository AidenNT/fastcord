use crate::BoxedResult;
use crate::clearance::{get_clearance_cookie, get_invisible};
use discord_client_utils::chrome_client_builder;
use log::warn;
use regex::Regex;
use std::sync::Arc;
use std::time::Duration;
use wreq::cookie::Jar;
use wreq::{Client, Proxy, redirect};

pub(crate) const DEFAULT_API_VERSION: u8 = 9;
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(20);

pub(crate) struct Bootstrap {
    pub client: Client,
    pub cookie_jar: Arc<Jar>,
    pub api_version: u8,
}

fn seed_default_cookies(jar: &Jar) {
    const DISCORD_URI: &str = "https://discord.com";

    jar.add(
        "OptanonAlertBoxClosed=2025-11-07T22:31:53.959Z; Domain=discord.com; Path=/",
        DISCORD_URI,
    );
    jar.add("locale=en-US; Domain=discord.com; Path=/", DISCORD_URI);
    jar.add(
        "OptanonConsent=isGpcEnabled; Domain=discord.com; Path=/",
        DISCORD_URI,
    );
}

pub(crate) fn build_emulated_client(proxy: Option<&str>) -> BoxedResult<(Client, Arc<Jar>)> {
    let cookie_jar = Arc::new(Jar::default());
    seed_default_cookies(&cookie_jar);
    // La emulación (Chrome/plataforma) vive en `discord_client_utils`, así
    // REST, clearance y super properties comparten una sola versión.
    let mut builder = chrome_client_builder()
        .gzip(true)
        .deflate(true)
        .brotli(true)
        .zstd(true)
        .cookie_provider(cookie_jar.clone())
        .redirect(redirect::Policy::default())
        .connect_timeout(CONNECT_TIMEOUT)
        .timeout(REQUEST_TIMEOUT);

    // Debug: para poder interceptar el tráfico con HTTP Toolkit / mitmproxy
    // sin pelearte con el almacén de certificados de Windows (wreq usa el
    // suyo propio, compilado con BoringSSL, y no lee el de Windows/Chrome
    // por más que ahí esté instalada la CA de la herramienta). Seteá la
    // variable de entorno ECORD_INSECURE_TLS=1 antes de correr el programa
    // mientras estés debuggeando, y sacala/apagala para uso normal.
    if crate::insecure_tls_enabled() {
        log::warn!(
            "ECORD_INSECURE_TLS=1: verificación de certificados TLS desactivada, solo para debug"
        );
        builder = builder.tls_cert_verification(false);
    }

    if let Some(proxy) = proxy {
        builder = builder.proxy(Proxy::all(proxy)?);
    }

    Ok((builder.build()?, cookie_jar))
}

pub(crate) fn build_bot_client(proxy: Option<&str>) -> BoxedResult<Client> {
    let mut builder = Client::builder()
        .user_agent(concat!(
            "DiscordBot (",
            env!("CARGO_PKG_REPOSITORY"),
            ", ",
            env!("CARGO_PKG_VERSION"),
            ")"
        ))
        .connect_timeout(CONNECT_TIMEOUT)
        .timeout(REQUEST_TIMEOUT);

    if let Some(proxy) = proxy {
        builder = builder.proxy(Proxy::all(proxy)?);
    }

    Ok(builder.build()?)
}

async fn fetch_app_shell(client: &Client) -> BoxedResult<String> {
    let url = "https://discord.com/channels/@me";
    let trace = crate::nettrace::begin("GET", url, "vendor::bootstrap (shell de la app web)");
    let resp = client
        .get(url)
        .orig_headers(crate::header_case::chrome_header_case())
        .send()
        .await;
    crate::nettrace::observe(&trace, &resp);
    let resp = resp?;

    Ok(resp.text().await?)
}

fn parse_api_version(body: &str) -> BoxedResult<u8> {
    let re = Regex::new(r#""API_VERSION":(\d+)"#).unwrap();

    if let Some(caps) = re.captures(body) {
        Ok(caps
            .get(1)
            .ok_or("Failed to find API version")?
            .as_str()
            .parse::<u8>()?)
    } else {
        Err(Box::from("Failed to find API version"))
    }
}

fn parse_challenge_token(body: &str) -> Option<String> {
    Regex::new(r#"r:'([a-f0-9]+)'"#)
        .unwrap()
        .captures(body)
        .and_then(|c| c.get(1))
        .map(|m| m.as_str().to_string())
}

async fn solve_clearance_from_body(client: &Client, body: &str) {
    match (get_invisible(client).await, parse_challenge_token(body)) {
        (Ok(invisible), Some(r)) => {
            if let Err(e) = get_clearance_cookie(client, invisible, r).await {
                warn!("cloudflare clearance failed (continuing without it): {}", e);
            }
        }
        (Ok(_), None) => warn!("cloudflare clearance skipped: challenge token r not found"),
        (Err(e), _) => {
            warn!(
                "cloudflare clearance setup failed (continuing without it): {}",
                e
            )
        }
    }
}

pub(crate) async fn solve_cloudflare_clearance(client: &Client) -> BoxedResult<()> {
    let body = fetch_app_shell(client).await?;
    solve_clearance_from_body(client, &body).await;
    Ok(())
}

pub(crate) async fn bootstrap_client(
    custom_api_version: Option<u8>,
    proxy: Option<&str>,
) -> BoxedResult<Bootstrap> {
    let (client, cookie_jar) = build_emulated_client(proxy)?;
    let body = fetch_app_shell(&client).await?;

    let api_version = match custom_api_version {
        Some(v) => v,
        None => parse_api_version(&body)?,
    };

    solve_clearance_from_body(&client, &body).await;

    Ok(Bootstrap {
        client,
        cookie_jar,
        api_version,
    })
}
