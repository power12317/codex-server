//! Native Images transport and completion validation, with immediate accepted events.
use super::*;
use eventsource_stream::Eventsource;
use futures::StreamExt;
use serde_json::Value;

impl<T: HttpTransport> ImagesClient<T> {
    pub async fn generate_live(
        &self,
        request: &ImageGenerationRequest,
        headers: HeaderMap,
    ) -> Result<(), ImageRequestError> {
        self.live("images/generations", request, headers).await
    }

    pub async fn edit_live(
        &self,
        request: &ImageEditRequest,
        headers: HeaderMap,
    ) -> Result<(), ImageRequestError> {
        self.live("images/edits", request, headers).await
    }

    async fn live<R: Serialize>(
        &self,
        path: &str,
        request: &R,
        headers: HeaderMap,
    ) -> Result<(), ImageRequestError> {
        let run = async {
            let value = to_value(request).map_err(|e| ApiError::InvalidRequest {
                message: e.to_string(),
            })?;
            let body = codex_client::EncodedJsonBody::encode(&value).map_err(|e| {
                ApiError::InvalidRequest {
                    message: e.to_string(),
                }
            })?;
            let mut response = self
                .session
                .stream_encoded_json_with(Method::POST, path, headers, Some(body), |request| {
                    request.headers.insert(
                        http::header::ACCEPT,
                        http::HeaderValue::from_static("application/json, text/event-stream"),
                    );
                })
                .await?;
            let sse = response
                .headers
                .get(http::header::CONTENT_TYPE)
                .and_then(|v| v.to_str().ok())
                .is_some_and(|v| v.to_ascii_lowercase().contains("text/event-stream"));
            if !sse {
                let mut bytes = Vec::new();
                while let Some(chunk) = response.bytes.next().await {
                    bytes.extend_from_slice(&chunk?);
                }
                // Image providers may return either inline data or a URL. Validate
                // the response's business envelope without imposing a b64-only DTO.
                let value: Value = serde_json::from_slice(&bytes)
                    .map_err(|e| ApiError::Stream(format!("invalid Images JSON: {e}")))?;
                image_error(&value, &String::from_utf8_lossy(&bytes))?;
                let envelope: ImageEnvelope = serde_json::from_value(value).map_err(|e| {
                    ApiError::Stream(format!("invalid Images response envelope: {e}"))
                })?;
                validate_results(&envelope.data)?;
                crate::observation::observe_image(&bytes);
                return Ok(());
            }
            let mut events = response.bytes.eventsource();
            while let Some(event) = events.next().await {
                let event =
                    event.map_err(|e| ApiError::Stream(format!("Images event stream: {e}")))?;
                if event.data == "[DONE]" {
                    continue;
                }
                let value: Value = serde_json::from_str(&event.data)
                    .map_err(|e| ApiError::Stream(format!("invalid Images event: {e}")))?;
                image_error(&value, &event.data)?;
                let kind = value
                    .get("type")
                    .and_then(Value::as_str)
                    .unwrap_or(&event.event);
                let terminal =
                    matches!(kind, "image_generation.completed" | "image_edit.completed");
                if terminal && !has_image(&value) {
                    let results: Vec<ImageResult> = serde_json::from_value(value["data"].clone())
                        .map_err(|e| {
                        ApiError::Stream(format!("invalid Images completed result: {e}"))
                    })?;
                    validate_results(&results)?;
                }
                let bytes = format!("data: {}\n\n", event.data);
                crate::observation::observe_image(bytes.as_bytes());
                if terminal {
                    return Ok(());
                }
            }
            Err(ApiError::Stream(
                "Images stream closed before an image completion event".into(),
            ))
        }
        .await;
        run.map_err(ImageRequestError::from_api_error)
    }
}

fn image_error(value: &Value, payload: &str) -> Result<(), ApiError> {
    if value.get("error").is_some_and(|error| !error.is_null())
        || matches!(
            value["type"].as_str(),
            Some("error" | "image_generation.failed" | "image_edit.failed")
        )
    {
        let error = ApiError::Stream(
            value
                .pointer("/error/message")
                .and_then(Value::as_str)
                .unwrap_or("Images request failed")
                .into(),
        );
        crate::observation::observe_failed_event(payload, &error);
        return Err(error);
    }
    Ok(())
}

#[derive(serde::Deserialize)]
struct ImageEnvelope {
    #[serde(rename = "created")]
    _created: u64,
    data: Vec<ImageResult>,
}

#[derive(serde::Deserialize)]
#[serde(untagged)]
enum ImageResult {
    Inline { b64_json: String },
    Url { url: String },
}

fn has_image(value: &Value) -> bool {
    ["b64_json", "url"].iter().any(|key| {
        value
            .get(key)
            .and_then(Value::as_str)
            .is_some_and(|image| !image.is_empty())
    })
}

fn validate_results(results: &[ImageResult]) -> Result<(), ApiError> {
    if results.is_empty()
        || results.iter().any(|result| match result {
            ImageResult::Inline { b64_json } => b64_json.is_empty(),
            ImageResult::Url { url } => url.is_empty(),
        })
    {
        return Err(ApiError::Stream(
            "Images response has no image result".into(),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn images_require_nonempty_results_and_allow_null_error_metadata() {
        for data in [json!([]), json!([{"b64_json":""}]), json!([{"url":""}])] {
            let results: Vec<ImageResult> = serde_json::from_value(data).unwrap();
            assert!(validate_results(&results).is_err());
        }
        for data in [
            json!([{"b64_json":"image"}]),
            json!([{"url":"https://example.test/image"}]),
        ] {
            let results: Vec<ImageResult> = serde_json::from_value(data).unwrap();
            assert!(validate_results(&results).is_ok());
        }
        assert!(image_error(&json!({"error":null}), "").is_ok());
        assert!(image_error(&json!({"error":{"message":"failed"}}), "failed").is_err());
    }
}
