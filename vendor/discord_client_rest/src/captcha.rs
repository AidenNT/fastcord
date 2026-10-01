use serde::Deserialize;
use std::error::Error;
use std::fmt::{Debug, Display};
use wreq::header::HeaderMap;

#[derive(Debug, Deserialize)]
pub struct CaptchaRequiredError {
    #[serde(default)]
    pub captcha_key: Vec<String>,
    pub captcha_sitekey: String,
    #[serde(default)]
    pub captcha_service: String,
    #[serde(default)]
    pub captcha_rqdata: String,
    #[serde(default)]
    pub captcha_rqtoken: String,
}
impl Display for CaptchaRequiredError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "Captcha required: captcha_key: {:?}, captcha_sitekey: {}, captcha_service: {}, captcha_rqdata: {}, captcha_rqtoken: {}",
            self.captcha_key,
            self.captcha_sitekey,
            self.captcha_service,
            self.captcha_rqdata,
            self.captcha_rqtoken
        )
    }
}

impl Error for CaptchaRequiredError {}

#[derive(Clone, Debug)]
pub struct SolvedCaptcha {
    pub key: String,
    pub rqtoken: String,
}

impl SolvedCaptcha {
    pub fn new(key: String, rqtoken: String) -> Self {
        Self { key, rqtoken }
    }

    pub fn add_headers(&self, headers: &mut HeaderMap) {
        if let Ok(value) = self.key.parse() {
            headers.insert("x-captcha-key", value);
        }
        if !self.rqtoken.is_empty() {
            if let Ok(value) = self.rqtoken.parse() {
                headers.insert("x-captcha-rqtoken", value);
            }
        }
    }
}
