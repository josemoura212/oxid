use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ShortenRequest {
    pub url: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ShortenResponse {
    pub code: String,
    pub short_url: String,
    pub long_url: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ErrorResponse {
    pub error: String,
}
