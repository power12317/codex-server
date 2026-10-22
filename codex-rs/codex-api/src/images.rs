use codex_protocol::models::ImageReference;
use serde::Deserialize;
use serde::Serialize;

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct ImageGenerationRequest {
    pub prompt: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub background: Option<ImageBackground>,
    pub model: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub n: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub quality: Option<ImageQuality>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub size: Option<String>,
    #[serde(flatten)]
    pub options: ImageRequestOptions,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct ImageEditRequest {
    pub images: Vec<ImageReference>,
    pub prompt: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub background: Option<ImageBackground>,
    pub model: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub n: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub quality: Option<ImageQuality>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub size: Option<String>,
    #[serde(flatten)]
    pub options: ImageRequestOptions,
}

/// Standalone image controls shared by the native unary and streaming backends.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct ImageRequestOptions {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mask: Option<ImageReference>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stream: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output_format: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output_compression: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub partial_images: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub response_format: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub input_fidelity: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub moderation: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub user: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ImageBackground {
    Transparent,
    Opaque,
    Auto,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ImageQuality {
    Low,
    Medium,
    High,
    Auto,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct ImageResponse {
    pub created: u64,
    pub data: Vec<ImageData>,
    #[serde(default)]
    pub background: Option<ImageBackground>,
    #[serde(default)]
    pub quality: Option<ImageQuality>,
    #[serde(default)]
    pub size: Option<String>,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct ImageData {
    pub b64_json: String,
    #[serde(default)]
    pub generation_id: Option<String>,
}
