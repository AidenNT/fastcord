use serde::Deserialize;
use std::error::Error;
use std::fmt::{Debug, Display};
use wreq::header::HeaderMap;

#[derive(Debug, Deserialize)]
pub struct CaptchaRequiredError {
    #[serde(default)]
    pub captcha_key: Vec<String>,
    /// `?string` según la doc: puede venir `null` (p. ej. reCAPTCHA).
    #[serde(default, deserialize_with = "null_as_default")]
    pub captcha_sitekey: String,
    #[serde(default, deserialize_with = "null_as_default")]
    pub captcha_service: String,
    #[serde(default, deserialize_with = "null_as_default")]
    pub captcha_session_id: String,
    #[serde(default, deserialize_with = "null_as_default")]
    pub captcha_rqdata: String,
    #[serde(default, deserialize_with = "null_as_default")]
    pub captcha_rqtoken: String,
    #[serde(default)]
    pub should_serve_invisible: bool,
}

fn null_as_default<'de, D>(deserializer: D) -> Result<String, D::Error>
where
    D: serde::Deserializer<'de>,
{
    Ok(Option::<String>::deserialize(deserializer)?.unwrap_or_default())
}
impl Display for CaptchaRequiredError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "Captcha required: captcha_key: {:?}, captcha_sitekey: {}, captcha_service: {}, captcha_session_id: {}",
            self.captcha_key, self.captcha_sitekey, self.captcha_service, self.captcha_session_id
        )
    }
}

impl Error for CaptchaRequiredError {}

#[derive(Clone, Debug)]
pub struct SolvedCaptcha {
    pub key: String,
    pub rqtoken: String,
    pub session_id: String,
}

impl SolvedCaptcha {
    pub fn new(key: String, rqtoken: String) -> Self {
        Self { key, rqtoken, session_id: String::new() }
    }

    /// `captcha_session_id` del desafío; si vino, la doc exige devolverlo en
    /// `X-Captcha-Session-Id`.
    pub fn with_session_id(mut self, session_id: String) -> Self {
        self.session_id = session_id;
        self
    }

    pub fn add_headers(&self, headers: &mut HeaderMap) {
        if let Ok(value) = self.key.parse() {
            headers.insert("x-captcha-key", value);
        }
        if !self.session_id.is_empty() {
            if let Ok(value) = self.session_id.parse() {
                headers.insert("x-captcha-session-id", value);
            }
        }
        if !self.rqtoken.is_empty() {
            if let Ok(value) = self.rqtoken.parse() {
                headers.insert("x-captcha-rqtoken", value);
            }
        }
    }
}
