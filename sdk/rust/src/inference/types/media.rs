use super::{AudioFormat, ImageFormat};

// ---------------------------------------------------------------------------
// Image generation types
// ---------------------------------------------------------------------------

/// Canonical image sizes understood by DALL-E and Imagen.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ImageSize {
    S256x256,
    S512x512,
    S1024x1024,
    /// Portrait (DALL-E 3, Imagen 9:16)
    S1024x1792,
    /// Landscape (DALL-E 3, Imagen 16:9)
    S1792x1024,
    /// Provider-specific size string (e.g. "768x768")
    Custom(String),
}

impl ImageSize {
    pub fn as_str(&self) -> &str {
        match self {
            Self::S256x256 => "256x256",
            Self::S512x512 => "512x512",
            Self::S1024x1024 => "1024x1024",
            Self::S1024x1792 => "1024x1792",
            Self::S1792x1024 => "1792x1024",
            Self::Custom(s) => s,
        }
    }
}

impl std::fmt::Display for ImageSize {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

impl ImageSize {
    /// Map to Imagen/Veo aspect ratio string.
    pub fn as_aspect_ratio(&self) -> &str {
        match self {
            Self::S1792x1024 => "16:9",
            Self::S1024x1792 => "9:16",
            _ => "1:1",
        }
    }
}

/// Image quality hint.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ImageQuality {
    Standard,
    /// HD quality (DALL-E 3)
    Hd,
    Custom(String),
}

impl ImageQuality {
    pub fn as_str(&self) -> &str {
        match self {
            Self::Standard => "standard",
            Self::Hd => "hd",
            Self::Custom(s) => s,
        }
    }
}

impl std::fmt::Display for ImageQuality {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Image style hint (DALL-E 3 only).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ImageStyle {
    /// Bold, saturated, stylized look.
    Vivid,
    /// More natural, less hyper-real look.
    Natural,
}

impl ImageStyle {
    pub fn as_str(&self) -> &str {
        match self {
            Self::Vivid => "vivid",
            Self::Natural => "natural",
        }
    }
}

impl std::fmt::Display for ImageStyle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Desired output format for generated images.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum ImageOutputFormat {
    /// Return a URL pointing to the image (default).
    #[default]
    Url,
    /// Return base64-encoded image data.
    B64Json,
}

impl ImageOutputFormat {
    pub fn as_str(&self) -> &str {
        match self {
            Self::Url => "url",
            Self::B64Json => "b64_json",
        }
    }
}

impl std::fmt::Display for ImageOutputFormat {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Request to generate one or more images from a text prompt.
#[derive(Debug, Clone)]
pub struct ImageGenerationRequest {
    pub model: String,
    pub prompt: String,
    /// Number of images to generate (default 1).
    pub n: Option<u32>,
    pub size: Option<ImageSize>,
    pub quality: Option<ImageQuality>,
    pub style: Option<ImageStyle>,
    pub output_format: ImageOutputFormat,
    pub user: Option<String>,
    /// Random seed for reproducibility (DALL-E, Imagen; not supported by all providers).
    pub seed: Option<u64>,
}

impl ImageGenerationRequest {
    pub fn new(model: impl Into<String>, prompt: impl Into<String>) -> Self {
        Self {
            model: model.into(),
            prompt: prompt.into(),
            n: None,
            size: None,
            quality: None,
            style: None,
            output_format: ImageOutputFormat::Url,
            user: None,
            seed: None,
        }
    }

    pub fn with_n(mut self, n: u32) -> Self {
        self.n = Some(n);
        self
    }

    pub fn with_size(mut self, size: ImageSize) -> Self {
        self.size = Some(size);
        self
    }

    pub fn with_quality(mut self, quality: ImageQuality) -> Self {
        self.quality = Some(quality);
        self
    }

    pub fn with_style(mut self, style: ImageStyle) -> Self {
        self.style = Some(style);
        self
    }

    pub fn with_output_format(mut self, format: ImageOutputFormat) -> Self {
        self.output_format = format;
        self
    }

    pub fn with_user(mut self, user: impl Into<String>) -> Self {
        self.user = Some(user.into());
        self
    }

    pub fn with_seed(mut self, seed: u64) -> Self {
        self.seed = Some(seed);
        self
    }
}

/// A single generated image.
#[derive(Debug, Clone)]
pub struct GeneratedImage {
    /// URL to the image (output_format = Url).
    pub url: Option<String>,
    /// Base64-encoded image data (output_format = B64Json or Imagen).
    pub b64_json: Option<String>,
    /// DALL-E 3 may return an enhanced version of the original prompt.
    pub revised_prompt: Option<String>,
}

/// Response from an image generation request.
#[derive(Debug, Clone)]
pub struct ImageGenerationResponse {
    pub images: Vec<GeneratedImage>,
}

/// Request to edit an existing image (DALL-E 2, gpt-image-1 inpainting).
#[derive(Debug, Clone)]
pub struct ImageEditRequest {
    pub model: String,
    /// Image to edit as raw bytes.
    pub image: Vec<u8>,
    /// Format of the image file (used for the multipart filename).
    pub image_format: ImageFormat,
    /// Optional mask (transparent areas indicate what to edit). PNG only. DALL-E 2 only.
    pub mask: Option<Vec<u8>>,
    pub prompt: String,
    pub n: Option<u32>,
    pub size: Option<ImageSize>,
    pub output_format: ImageOutputFormat,
    pub user: Option<String>,
}

impl ImageEditRequest {
    pub fn new(
        model: impl Into<String>,
        image: Vec<u8>,
        image_format: ImageFormat,
        prompt: impl Into<String>,
    ) -> Self {
        Self {
            model: model.into(),
            image,
            image_format,
            mask: None,
            prompt: prompt.into(),
            n: None,
            size: None,
            output_format: ImageOutputFormat::Url,
            user: None,
        }
    }

    pub fn with_mask(mut self, mask: Vec<u8>) -> Self {
        self.mask = Some(mask);
        self
    }

    pub fn with_n(mut self, n: u32) -> Self {
        self.n = Some(n);
        self
    }

    pub fn with_size(mut self, size: ImageSize) -> Self {
        self.size = Some(size);
        self
    }

    pub fn with_output_format(mut self, format: ImageOutputFormat) -> Self {
        self.output_format = format;
        self
    }

    pub fn with_user(mut self, user: impl Into<String>) -> Self {
        self.user = Some(user.into());
        self
    }
}

// ---------------------------------------------------------------------------
// Video generation types
// ---------------------------------------------------------------------------

/// Aspect ratio for generated videos.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VideoAspectRatio {
    Landscape16x9,
    Portrait9x16,
    Square1x1,
    Custom(String),
}

impl VideoAspectRatio {
    pub fn as_str(&self) -> &str {
        match self {
            Self::Landscape16x9 => "16:9",
            Self::Portrait9x16 => "9:16",
            Self::Square1x1 => "1:1",
            Self::Custom(s) => s,
        }
    }
}

impl std::fmt::Display for VideoAspectRatio {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Resolution for generated videos.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VideoResolution {
    P720,
    P1080,
    Custom(String),
}

impl VideoResolution {
    pub fn as_str(&self) -> &str {
        match self {
            Self::P720 => "720p",
            Self::P1080 => "1080p",
            Self::Custom(s) => s,
        }
    }
}

impl std::fmt::Display for VideoResolution {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Request to generate one or more videos from a text prompt.
#[derive(Debug, Clone)]
pub struct VideoGenerationRequest {
    pub model: String,
    pub prompt: String,
    /// Number of videos to generate (default 1).
    pub n: Option<u32>,
    /// Video duration in seconds.
    pub duration_secs: Option<u32>,
    pub aspect_ratio: Option<VideoAspectRatio>,
    pub resolution: Option<VideoResolution>,
    /// S3 URI for async output (required for Bedrock Nova Reel: `s3://bucket/prefix`).
    pub output_storage_uri: Option<String>,
    /// Random seed for reproducibility (not supported by all providers).
    pub seed: Option<u64>,
}

impl VideoGenerationRequest {
    pub fn new(model: impl Into<String>, prompt: impl Into<String>) -> Self {
        Self {
            model: model.into(),
            prompt: prompt.into(),
            n: None,
            duration_secs: None,
            aspect_ratio: None,
            resolution: None,
            output_storage_uri: None,
            seed: None,
        }
    }

    pub fn with_n(mut self, n: u32) -> Self {
        self.n = Some(n);
        self
    }

    pub fn with_duration_secs(mut self, secs: u32) -> Self {
        self.duration_secs = Some(secs);
        self
    }

    pub fn with_aspect_ratio(mut self, ar: VideoAspectRatio) -> Self {
        self.aspect_ratio = Some(ar);
        self
    }

    pub fn with_resolution(mut self, res: VideoResolution) -> Self {
        self.resolution = Some(res);
        self
    }

    /// Set the S3 output URI (required for Bedrock Nova Reel).
    pub fn with_output_storage_uri(mut self, uri: impl Into<String>) -> Self {
        self.output_storage_uri = Some(uri.into());
        self
    }

    pub fn with_seed(mut self, seed: u64) -> Self {
        self.seed = Some(seed);
        self
    }
}

/// A single generated video.
#[derive(Debug, Clone)]
pub struct GeneratedVideo {
    /// Download URI (GCS URL for Veo, CDN URL for others).
    pub uri: Option<String>,
    /// Base64-encoded video data (if returned inline).
    pub b64_json: Option<String>,
    pub duration_secs: Option<f64>,
}

/// Response from a video generation request.
#[derive(Debug, Clone)]
pub struct VideoGenerationResponse {
    pub videos: Vec<GeneratedVideo>,
}

// ---------------------------------------------------------------------------
// TTS / Transcription types
// ---------------------------------------------------------------------------

/// Text-to-speech request.
#[derive(Debug, Clone)]
pub struct SpeechRequest {
    pub model: String,
    pub input: String,
    pub voice: String,
    /// Output audio format. None = provider default (mp3).
    pub response_format: Option<AudioFormat>,
    /// Playback speed multiplier (0.25–4.0).
    pub speed: Option<f64>,
    /// Speaking style instructions (supported by `gpt-4o-mini-tts`).
    pub instructions: Option<String>,
}

impl SpeechRequest {
    pub fn new(
        model: impl Into<String>,
        input: impl Into<String>,
        voice: impl Into<String>,
    ) -> Self {
        Self {
            model: model.into(),
            input: input.into(),
            voice: voice.into(),
            response_format: None,
            speed: None,
            instructions: None,
        }
    }

    pub fn with_format(mut self, f: AudioFormat) -> Self {
        self.response_format = Some(f);
        self
    }

    pub fn with_speed(mut self, s: f64) -> Self {
        self.speed = Some(s);
        self
    }

    pub fn with_instructions(mut self, instructions: impl Into<String>) -> Self {
        self.instructions = Some(instructions.into());
        self
    }
}

/// Text-to-speech response.
#[derive(Debug, Clone)]
pub struct SpeechResponse {
    pub audio: Vec<u8>,
    /// Actual format returned (mp3 if None was passed in request).
    pub format: AudioFormat,
}

/// Granularity level for transcription timestamps.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TimestampGranularity {
    /// Per-word timestamps in the response `words` array.
    Word,
    /// Per-segment timestamps in the response `segments` array.
    Segment,
}

impl TimestampGranularity {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Word => "word",
            Self::Segment => "segment",
        }
    }
}

/// Word-level timestamp from a transcription.
#[derive(Debug, Clone)]
pub struct TranscriptionWord {
    pub word: String,
    pub start: f64,
    pub end: f64,
}

/// Segment-level detail from a transcription (Whisper verbose_json).
#[derive(Debug, Clone)]
pub struct TranscriptionSegment {
    pub id: u32,
    pub start: f64,
    pub end: f64,
    pub text: String,
    pub temperature: f64,
    pub avg_logprob: f64,
    pub no_speech_prob: f64,
}

/// Speech-to-text transcription request.
#[derive(Debug, Clone)]
pub struct TranscriptionRequest {
    pub model: String,
    pub audio: Vec<u8>,
    pub format: AudioFormat,
    pub language: Option<String>,
    pub prompt: Option<String>,
    pub temperature: Option<f64>,
    /// Request word- and/or segment-level timestamps. Requires `verbose_json` response format.
    pub timestamp_granularities: Option<Vec<TimestampGranularity>>,
}

impl TranscriptionRequest {
    pub fn new(model: impl Into<String>, audio: Vec<u8>, format: AudioFormat) -> Self {
        Self {
            model: model.into(),
            audio,
            format,
            language: None,
            prompt: None,
            temperature: None,
            timestamp_granularities: None,
        }
    }

    pub fn with_language(mut self, lang: impl Into<String>) -> Self {
        self.language = Some(lang.into());
        self
    }

    pub fn with_prompt(mut self, p: impl Into<String>) -> Self {
        self.prompt = Some(p.into());
        self
    }

    pub fn with_temperature(mut self, t: f64) -> Self {
        self.temperature = Some(t);
        self
    }

    pub fn with_timestamp_granularities(mut self, g: Vec<TimestampGranularity>) -> Self {
        self.timestamp_granularities = Some(g);
        self
    }
}

/// Speech-to-text transcription response.
#[derive(Debug, Clone)]
pub struct TranscriptionResponse {
    pub text: String,
    pub language: Option<String>,
    pub duration_secs: Option<f64>,
    /// Per-word timestamps (populated when `TimestampGranularity::Word` is requested).
    pub words: Vec<TranscriptionWord>,
    /// Per-segment details (populated when `TimestampGranularity::Segment` is requested).
    pub segments: Vec<TranscriptionSegment>,
}
