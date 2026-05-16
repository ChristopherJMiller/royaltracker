#[derive(Debug, thiserror::Error)]
pub enum ApiError {
    #[error("http: {0}")]
    Http(#[from] wreq::Error),

    #[error("status {status}: {body}")]
    Status { status: u16, body: String },

    #[error("auth: missing or invalid JWT")]
    BadJwt,

    #[error("serde: {0}")]
    Serde(#[from] serde_json::Error),

    #[error("url: {0}")]
    Url(#[from] url::ParseError),

    #[error("missing field: {0}")]
    MissingField(&'static str),

    #[error("base64: {0}")]
    Base64(#[from] base64::DecodeError),
}
