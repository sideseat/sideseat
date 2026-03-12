use std::sync::Arc;

use serde_json::{Value, json};

use crate::{
    error::ProviderError,
    providers::sse::check_response,
    types::{
        AudioFormat, EmbeddingRequest, EmbeddingResponse, GeneratedImage, ImageEditRequest,
        ImageFormat, ImageGenerationRequest, ImageGenerationResponse, ModelInfo,
        ModerationCategories, ModerationCategoryScores, ModerationRequest, ModerationResponse,
        ModerationResult, SpeechRequest, SpeechResponse, StopReason, TranscriptionRequest,
        TranscriptionResponse, TranscriptionSegment, TranscriptionWord, Usage,
    },
};

// ---------------------------------------------------------------------------
// Shared OpenAI HTTP client + base URL bundle
// ---------------------------------------------------------------------------

/// Shared HTTP client and credentials used by both OpenAI providers.
pub(crate) struct OpenAIInnerClient {
    pub api_key: String,
    pub client: Arc<reqwest::Client>,
    pub api_base: String,
}

impl OpenAIInnerClient {
    pub fn new(
        api_key: impl Into<String>,
        client: Arc<reqwest::Client>,
        api_base: impl Into<String>,
    ) -> Self {
        Self {
            api_key: api_key.into(),
            client,
            api_base: api_base.into(),
        }
    }

    // ---------------------------------------------------------------------------
    // list_models
    // ---------------------------------------------------------------------------

    pub async fn list_models(&self) -> Result<Vec<ModelInfo>, ProviderError> {
        let url = format!("{}/models", self.api_base);
        let resp = self
            .client
            .get(&url)
            .bearer_auth(&self.api_key)
            .send()
            .await?;
        let resp = check_response(resp).await?;
        let json: Value = resp.json().await?;

        let mut models = Vec::new();
        if let Some(arr) = json["data"].as_array() {
            for item in arr {
                let id = item["id"].as_str().unwrap_or("").to_string();
                if id.is_empty() {
                    continue;
                }
                models.push(ModelInfo {
                    id,
                    display_name: None,
                    description: None,
                    created_at: item["created"].as_u64(),
                });
            }
        }
        Ok(models)
    }

    // ---------------------------------------------------------------------------
    // embed
    // ---------------------------------------------------------------------------

    pub async fn embed(
        &self,
        request: EmbeddingRequest,
    ) -> Result<EmbeddingResponse, ProviderError> {
        let url = format!("{}/embeddings", self.api_base);
        let mut body = json!({
            "model": request.model,
            "input": request.inputs,
        });
        if let Some(dims) = request.dimensions {
            body["dimensions"] = json!(dims);
        }

        let resp = self
            .client
            .post(&url)
            .bearer_auth(&self.api_key)
            .json(&body)
            .send()
            .await?;
        let resp = check_response(resp).await?;
        let json: Value = resp.json().await?;

        let mut embeddings: Vec<Vec<f32>> = Vec::new();
        if let Some(arr) = json["data"].as_array() {
            for item in arr {
                if let Some(vec_arr) = item["embedding"].as_array() {
                    let vec: Vec<f32> = vec_arr
                        .iter()
                        .filter_map(|v| v.as_f64().map(|f| f as f32))
                        .collect();
                    embeddings.push(vec);
                }
            }
        }

        let usage = parse_usage(&json["usage"]);
        let returned_model = json["model"].as_str().map(|s| s.to_string());

        Ok(EmbeddingResponse {
            embeddings,
            model: returned_model,
            usage,
        })
    }

    // ---------------------------------------------------------------------------
    // generate_image
    // ---------------------------------------------------------------------------

    pub async fn generate_image(
        &self,
        request: ImageGenerationRequest,
    ) -> Result<ImageGenerationResponse, ProviderError> {
        let url = format!("{}/images/generations", self.api_base);

        let mut body = json!({
            "model": request.model,
            "prompt": request.prompt,
            "response_format": request.output_format.as_str(),
        });
        if let Some(n) = request.n {
            body["n"] = json!(n);
        }
        if let Some(size) = &request.size {
            body["size"] = json!(size.as_str());
        }
        if let Some(quality) = &request.quality {
            body["quality"] = json!(quality.as_str());
        }
        if let Some(style) = &request.style {
            body["style"] = json!(style.as_str());
        }
        if let Some(user) = &request.user {
            body["user"] = json!(user);
        }
        if let Some(seed) = request.seed {
            body["seed"] = json!(seed);
        }

        let resp = self
            .client
            .post(&url)
            .bearer_auth(&self.api_key)
            .json(&body)
            .send()
            .await?;
        let resp = check_response(resp).await?;
        let json: Value = resp.json().await?;

        let images = json["data"]
            .as_array()
            .unwrap_or(&vec![])
            .iter()
            .map(|item| GeneratedImage {
                url: item["url"].as_str().map(|s| s.to_string()),
                b64_json: item["b64_json"].as_str().map(|s| s.to_string()),
                revised_prompt: item["revised_prompt"].as_str().map(|s| s.to_string()),
            })
            .collect();

        Ok(ImageGenerationResponse { images })
    }

    // ---------------------------------------------------------------------------
    // edit_image
    // ---------------------------------------------------------------------------

    pub async fn edit_image(
        &self,
        request: ImageEditRequest,
    ) -> Result<ImageGenerationResponse, ProviderError> {
        let url = format!("{}/images/edits", self.api_base);

        let img_ext = match request.image_format {
            ImageFormat::Jpeg => "jpg",
            ImageFormat::Png => "png",
            ImageFormat::Webp => "webp",
            _ => "png",
        };
        let image_part = reqwest::multipart::Part::bytes(request.image)
            .file_name(format!("image.{img_ext}"))
            .mime_str("application/octet-stream")
            .map_err(|e| ProviderError::Serialization(e.to_string()))?;

        let mut form = reqwest::multipart::Form::new()
            .part("image", image_part)
            .text("model", request.model)
            .text("prompt", request.prompt)
            .text(
                "response_format",
                request.output_format.as_str().to_string(),
            );

        if let Some(mask) = request.mask {
            let mask_part = reqwest::multipart::Part::bytes(mask)
                .file_name("mask.png")
                .mime_str("application/octet-stream")
                .map_err(|e| ProviderError::Serialization(e.to_string()))?;
            form = form.part("mask", mask_part);
        }
        if let Some(n) = request.n {
            form = form.text("n", n.to_string());
        }
        if let Some(size) = &request.size {
            form = form.text("size", size.as_str().to_string());
        }
        if let Some(user) = request.user {
            form = form.text("user", user);
        }

        let resp = self
            .client
            .post(&url)
            .bearer_auth(&self.api_key)
            .multipart(form)
            .send()
            .await?;
        let resp = check_response(resp).await?;
        let json: Value = resp.json().await?;

        let images = json["data"]
            .as_array()
            .unwrap_or(&vec![])
            .iter()
            .map(|item| GeneratedImage {
                url: item["url"].as_str().map(|s| s.to_string()),
                b64_json: item["b64_json"].as_str().map(|s| s.to_string()),
                revised_prompt: item["revised_prompt"].as_str().map(|s| s.to_string()),
            })
            .collect();

        Ok(ImageGenerationResponse { images })
    }

    // ---------------------------------------------------------------------------
    // generate_speech
    // ---------------------------------------------------------------------------

    pub async fn generate_speech(
        &self,
        request: SpeechRequest,
    ) -> Result<SpeechResponse, ProviderError> {
        let url = format!("{}/audio/speech", self.api_base);

        let mut body = serde_json::json!({
            "model": request.model,
            "input": request.input,
            "voice": request.voice,
        });
        let format = request.response_format.clone().unwrap_or(AudioFormat::Mp3);
        let format_str = match &format {
            AudioFormat::Mp3 => "mp3",
            AudioFormat::Wav => "wav",
            AudioFormat::Aac => "aac",
            AudioFormat::Flac => "flac",
            AudioFormat::Ogg => "ogg",
            AudioFormat::Opus => "opus",
            _ => "mp3",
        };
        body["response_format"] = serde_json::json!(format_str);
        if let Some(speed) = request.speed {
            body["speed"] = serde_json::json!(speed);
        }
        if let Some(instructions) = &request.instructions {
            body["instructions"] = serde_json::json!(instructions);
        }

        let resp = self
            .client
            .post(&url)
            .bearer_auth(&self.api_key)
            .json(&body)
            .send()
            .await?;
        let resp = check_response(resp).await?;
        let audio = resp
            .bytes()
            .await
            .map_err(|e| ProviderError::Network(e.to_string()))?
            .to_vec();

        Ok(SpeechResponse { audio, format })
    }

    // ---------------------------------------------------------------------------
    // transcribe
    // ---------------------------------------------------------------------------

    pub async fn transcribe(
        &self,
        request: TranscriptionRequest,
    ) -> Result<TranscriptionResponse, ProviderError> {
        let url = format!("{}/audio/transcriptions", self.api_base);

        let ext = audio_format_ext(&request.format);
        let filename = format!("audio.{ext}");

        let part = reqwest::multipart::Part::bytes(request.audio)
            .file_name(filename)
            .mime_str("application/octet-stream")
            .map_err(|e| ProviderError::Serialization(e.to_string()))?;

        let mut form = reqwest::multipart::Form::new()
            .part("file", part)
            .text("model", request.model)
            .text("response_format", "verbose_json");

        if let Some(lang) = request.language {
            form = form.text("language", lang);
        }
        if let Some(prompt) = request.prompt {
            form = form.text("prompt", prompt);
        }
        if let Some(temp) = request.temperature {
            form = form.text("temperature", temp.to_string());
        }
        if let Some(granularities) = request.timestamp_granularities {
            for g in &granularities {
                form = form.text("timestamp_granularities[]", g.as_str().to_string());
            }
        }

        let resp = self
            .client
            .post(&url)
            .bearer_auth(&self.api_key)
            .multipart(form)
            .send()
            .await?;
        let resp = check_response(resp).await?;
        let json: serde_json::Value = resp.json().await?;

        Ok(parse_transcription_response(&json))
    }

    // ---------------------------------------------------------------------------
    // translate
    // ---------------------------------------------------------------------------

    pub async fn translate(
        &self,
        request: TranscriptionRequest,
    ) -> Result<TranscriptionResponse, ProviderError> {
        let url = format!("{}/audio/translations", self.api_base);

        let ext = audio_format_ext(&request.format);
        let filename = format!("audio.{ext}");

        let part = reqwest::multipart::Part::bytes(request.audio)
            .file_name(filename)
            .mime_str("application/octet-stream")
            .map_err(|e| ProviderError::Serialization(e.to_string()))?;

        let mut form = reqwest::multipart::Form::new()
            .part("file", part)
            .text("model", request.model)
            .text("response_format", "verbose_json");

        // Note: translations always produce English output; no `language` param
        if let Some(prompt) = request.prompt {
            form = form.text("prompt", prompt);
        }
        if let Some(temp) = request.temperature {
            form = form.text("temperature", temp.to_string());
        }

        let resp = self
            .client
            .post(&url)
            .bearer_auth(&self.api_key)
            .multipart(form)
            .send()
            .await?;
        let resp = check_response(resp).await?;
        let json: serde_json::Value = resp.json().await?;

        Ok(parse_transcription_response(&json))
    }

    // ---------------------------------------------------------------------------
    // moderate
    // ---------------------------------------------------------------------------

    pub async fn moderate(
        &self,
        request: ModerationRequest,
    ) -> Result<ModerationResponse, ProviderError> {
        let url = format!("{}/moderations", self.api_base);

        let input: Value = if request.input.len() == 1 {
            json!(request.input[0])
        } else {
            json!(request.input)
        };

        let mut body = json!({ "input": input });
        if let Some(model) = request.model {
            body["model"] = json!(model);
        }

        let resp = self
            .client
            .post(&url)
            .bearer_auth(&self.api_key)
            .json(&body)
            .send()
            .await?;
        let resp = check_response(resp).await?;
        let json: Value = resp.json().await?;

        let id = json["id"].as_str().unwrap_or("").to_string();
        let model = json["model"].as_str().unwrap_or("").to_string();

        let results = json["results"]
            .as_array()
            .unwrap_or(&vec![])
            .iter()
            .map(|r| {
                let categories: ModerationCategories =
                    serde_json::from_value(r["categories"].clone()).unwrap_or_default();
                let category_scores: ModerationCategoryScores =
                    serde_json::from_value(r["category_scores"].clone()).unwrap_or_default();
                ModerationResult {
                    flagged: r["flagged"].as_bool().unwrap_or(false),
                    categories,
                    category_scores,
                }
            })
            .collect();

        Ok(ModerationResponse { id, model, results })
    }
}

// ---------------------------------------------------------------------------
// Shared helpers
// ---------------------------------------------------------------------------

pub(crate) fn audio_format_ext(format: &AudioFormat) -> &'static str {
    match format {
        AudioFormat::Mp3 => "mp3",
        AudioFormat::Wav => "wav",
        AudioFormat::Aac => "aac",
        AudioFormat::Flac => "flac",
        AudioFormat::Ogg => "ogg",
        AudioFormat::Opus => "opus",
        AudioFormat::M4a => "m4a",
        AudioFormat::Webm => "webm",
        AudioFormat::Aiff => "aiff",
        AudioFormat::Pcm16 => "pcm16",
    }
}

pub(crate) fn parse_transcription_response(json: &serde_json::Value) -> TranscriptionResponse {
    let words = json["words"]
        .as_array()
        .unwrap_or(&vec![])
        .iter()
        .map(|w| TranscriptionWord {
            word: w["word"].as_str().unwrap_or("").to_string(),
            start: w["start"].as_f64().unwrap_or(0.0),
            end: w["end"].as_f64().unwrap_or(0.0),
        })
        .collect();

    let segments = json["segments"]
        .as_array()
        .unwrap_or(&vec![])
        .iter()
        .map(|s| TranscriptionSegment {
            id: s["id"].as_u64().unwrap_or(0) as u32,
            start: s["start"].as_f64().unwrap_or(0.0),
            end: s["end"].as_f64().unwrap_or(0.0),
            text: s["text"].as_str().unwrap_or("").to_string(),
            temperature: s["temperature"].as_f64().unwrap_or(0.0),
            avg_logprob: s["avg_logprob"].as_f64().unwrap_or(0.0),
            no_speech_prob: s["no_speech_prob"].as_f64().unwrap_or(0.0),
        })
        .collect();

    TranscriptionResponse {
        text: json["text"].as_str().unwrap_or("").to_string(),
        language: json["language"].as_str().map(|s| s.to_string()),
        duration_secs: json["duration"].as_f64(),
        words,
        segments,
    }
}

/// Parse OpenAI Chat Completions usage object.
pub(crate) fn parse_usage(usage: &Value) -> Usage {
    Usage {
        input_tokens: usage["prompt_tokens"].as_u64().unwrap_or(0),
        output_tokens: usage["completion_tokens"].as_u64().unwrap_or(0),
        cache_read_tokens: usage["prompt_tokens_details"]["cached_tokens"]
            .as_u64()
            .unwrap_or(0),
        reasoning_tokens: usage["completion_tokens_details"]["reasoning_tokens"]
            .as_u64()
            .unwrap_or(0),
        ..Default::default()
    }
}

pub(crate) fn parse_finish_reason(reason: &str) -> StopReason {
    match reason {
        "stop" => StopReason::EndTurn,
        "length" => StopReason::MaxTokens,
        "tool_calls" | "function_call" => StopReason::ToolUse,
        "content_filter" => StopReason::ContentFilter,
        other => StopReason::Other(other.to_string()),
    }
}
