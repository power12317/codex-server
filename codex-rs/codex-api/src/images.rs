use codex_protocol::models::ImageReference;
use serde::Deserialize;
use serde::Serialize;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
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

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
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
    /// Complete caller business options beyond the typed native image fields.
    #[serde(default, flatten)]
    pub extensions: serde_json::Map<String, serde_json::Value>,
}

impl ImageRequestOptions {
    /// Called at the input boundary; native sends still serialize their own DTO.
    pub fn retain_caller_business_options(&mut self) {
        self.extensions
            .retain(|key, _| !crate::image_inference::worker_owned_field(key));
    }
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

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;
    use serde_json::json;

    #[test]
    fn native_image_input_preserves_all_fields_and_future_business_options() {
        let expected = ImageEditRequest {
            images: vec![ImageReference::File { file_id: "file-fixture".into() }],
            prompt: "fixture".into(), background: Some(ImageBackground::Transparent),
            model: "gpt-image-2".into(), n: Some(1), quality: Some(ImageQuality::Low),
            size: Some("1024x1024".into()),
            options: ImageRequestOptions {
                mask: Some(ImageReference::File { file_id: "file-mask".into() }),
                stream: Some(false), output_format: Some("webp".into()),
                output_compression: Some(70), partial_images: Some(2),
                response_format: Some("b64_json".into()), input_fidelity: Some("high".into()),
                moderation: Some("auto".into()), user: Some("fixture".into()),
                extensions: json!({"future_option":{"large":9007199254740993u64,"nullable":null,"enabled":false},"repeat":["a","b"]}).as_object().unwrap().clone(),
            },
        };
        let wire = serde_json::to_value(&expected).unwrap();
        let mut input = wire.clone();
        input["headers"] = json!({"authorization":"caller-secret"});
        input["client_metadata"] = json!({"session_id":"caller"});
        let mut actual: ImageEditRequest = serde_json::from_value(input).unwrap();
        actual.options.retain_caller_business_options();
        assert_eq!(serde_json::to_value(actual).unwrap(), wire);
    }
}
