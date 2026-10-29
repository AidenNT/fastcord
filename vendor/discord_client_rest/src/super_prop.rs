use base64::{Engine as _, engine::general_purpose::STANDARD};
use discord_client_structs::structs::client::{BuildNumbers, ClientSession};
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
struct SuperProps {
    os: String,
    browser: String,
    device: String,
    system_locale: String,
    has_client_mods: bool,
    browser_user_agent: String,
    browser_version: String,
    os_version: String,
    referrer: String,
    referring_domain: String,
    referrer_current: String,
    referring_domain_current: String,
    release_channel: String,
    client_build_number: u32,
    // El cliente web no envía `native_build_number` (solo el de escritorio):
    // si es None, la clave se omite en vez de serializarse como null.
    #[serde(skip_serializing_if = "Option::is_none")]
    native_build_number: Option<u32>,
    client_event_source: Option<String>,
    client_launch_id: String,
    launch_signature: String,
    client_app_state: String,
    client_heartbeat_session_id: String,
}

pub fn build_super_props(build_numbers: BuildNumbers, client_session: ClientSession) -> String {
    let props = SuperProps {
        os: "Windows".to_string(),
        browser: "Chrome".to_string(),
        device: "".to_string(),
        system_locale: "en-US".to_string(),
        has_client_mods: false,
        browser_user_agent: discord_client_utils::USER_AGENT.to_string(),
        browser_version: discord_client_utils::CHROME_VERSION.to_string(),
        os_version: "10".to_string(),
        referrer: "".to_string(),
        referring_domain: "".to_string(),
        referrer_current: "".to_string(),
        referring_domain_current: "".to_string(),
        release_channel: "stable".to_string(),
        client_build_number: build_numbers.client_build_number,
        native_build_number: build_numbers.native_build_number,
        client_event_source: None,
        client_launch_id: client_session.client_launch_id.to_string(),
        launch_signature: client_session.launch_signature.to_string(),
        client_app_state: "focused".to_string(),
        client_heartbeat_session_id: client_session.client_heartbeat_session_id.to_string(),
    };

    let json_string = serde_json::to_string(&props).unwrap();
    STANDARD.encode(json_string)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn decode(b64: &str) -> serde_json::Value {
        let raw = STANDARD.decode(b64).unwrap();
        serde_json::from_slice(&raw).unwrap()
    }

    #[test]
    fn web_super_props_shape() {
        let b64 = build_super_props(BuildNumbers::new(630_444, None), ClientSession::new());
        let v = decode(&b64);
        assert_eq!(v["has_client_mods"], false);
        assert_eq!(v["browser"], "Chrome");
        assert!(v.get("native_build_number").is_none());
        assert!(v["client_heartbeat_session_id"].is_string());
        // has_client_mods va justo después de system_locale
        let raw = String::from_utf8(STANDARD.decode(&b64).unwrap()).unwrap();
        assert!(raw.find("\"system_locale\"").unwrap() < raw.find("\"has_client_mods\"").unwrap());
        assert!(raw.find("\"has_client_mods\"").unwrap() < raw.find("\"browser_user_agent\"").unwrap());
    }
}
