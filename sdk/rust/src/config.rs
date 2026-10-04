//! Options, and the settings they resolve to: explicit options win over environment variables,
//! which win over defaults.

use std::collections::BTreeMap;
use std::fmt;

use opentelemetry::KeyValue;
use opentelemetry_sdk::trace::SpanProcessor;
use url::Url;

use crate::Error;

/// The server [`init`](crate::init) exports to when nothing else is configured.
pub const DEFAULT_ENDPOINT: &str = "http://127.0.0.1:5388";
/// The project that receives telemetry when nothing else is configured.
pub const DEFAULT_PROJECT: &str = "default";
const DEFAULT_SERVICE_NAME: &str = "sideseat-app";

/// What `encodeURIComponent` escapes, so every SDK puts a project into the path the same way.
const COMPONENT: &percent_encoding::AsciiSet = &percent_encoding::NON_ALPHANUMERIC
    .remove(b'-')
    .remove(b'_')
    .remove(b'.')
    .remove(b'!')
    .remove(b'~')
    .remove(b'*')
    .remove(b'\'')
    .remove(b'(')
    .remove(b')');

/// Options for [`init`](crate::init). Every option falls back to its environment variable, then to
/// a default.
#[derive(Default)]
pub struct Options {
    endpoint: Option<String>,
    project: Option<String>,
    api_key: Option<String>,
    service_name: Option<String>,
    service_version: Option<String>,
    capture_content: Option<bool>,
    disabled: Option<bool>,
    debug: Option<bool>,
    export: Option<bool>,
    logs: Option<bool>,
    resource_attributes: Vec<KeyValue>,
    span_processors: Vec<Box<dyn SpanProcessor>>,
}

impl Options {
    pub fn new() -> Self {
        Self::default()
    }

    /// A SideSeat server URL, or an OTLP base URL that already has a path. `SIDESEAT_ENDPOINT`,
    /// then `OTEL_EXPORTER_OTLP_ENDPOINT`.
    pub fn endpoint(mut self, endpoint: impl Into<String>) -> Self {
        self.endpoint = Some(endpoint.into());
        self
    }

    /// The project that receives the telemetry. `SIDESEAT_PROJECT_ID`.
    pub fn project(mut self, project: impl Into<String>) -> Self {
        self.project = Some(project.into());
        self
    }

    /// Sent as `Authorization: Bearer <key>`. `SIDESEAT_API_KEY`.
    pub fn api_key(mut self, key: impl Into<String>) -> Self {
        self.api_key = Some(key.into());
        self
    }

    /// `service.name`. `OTEL_SERVICE_NAME`.
    pub fn service_name(mut self, name: impl Into<String>) -> Self {
        self.service_name = Some(name.into());
        self
    }

    /// `service.version`. `OTEL_SERVICE_VERSION`.
    pub fn service_version(mut self, version: impl Into<String>) -> Self {
        self.service_version = Some(version.into());
        self
    }

    /// Whether prompts, responses, and tool payloads are recorded. On by default.
    /// `SIDESEAT_CAPTURE_CONTENT`.
    pub fn capture_content(mut self, capture: bool) -> Self {
        self.capture_content = Some(capture);
        self
    }

    /// Configure nothing: spans are non-recording and nothing is exported. `SIDESEAT_DISABLED`.
    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = Some(disabled);
        self
    }

    /// Print the resolved configuration to stderr at `init`. `SIDESEAT_DEBUG`.
    pub fn debug(mut self, debug: bool) -> Self {
        self.debug = Some(debug);
        self
    }

    /// Send telemetry over OTLP. Turning it off is useful with [`span_processor`](Self::span_processor)
    /// in tests.
    pub fn export(mut self, export: bool) -> Self {
        self.export = Some(export);
        self
    }

    /// Export OpenTelemetry log records. On by default.
    pub fn logs(mut self, logs: bool) -> Self {
        self.logs = Some(logs);
        self
    }

    /// An extra attribute on the resource of every signal.
    pub fn resource_attribute(mut self, attribute: KeyValue) -> Self {
        self.resource_attributes.push(attribute);
        self
    }

    /// An extra span processor, run after correlation and before export.
    pub fn span_processor(mut self, processor: impl SpanProcessor + 'static) -> Self {
        self.span_processors.push(Box::new(processor));
        self
    }

    pub(crate) fn resolve(self) -> Result<(Settings, Vec<Box<dyn SpanProcessor>>), Error> {
        self.resolve_with(|name| std::env::var(name).ok())
    }

    fn resolve_with(
        self,
        env: impl Fn(&str) -> Option<String>,
    ) -> Result<(Settings, Vec<Box<dyn SpanProcessor>>), Error> {
        let text = |explicit: Option<String>, name: &str| {
            explicit.or_else(|| env(name).filter(|value| !value.trim().is_empty()))
        };
        let flag = |explicit: Option<bool>, name: &str, default: bool| match explicit {
            Some(value) => Ok(value),
            None => env(name).map_or(Ok(default), |raw| parse_flag(name, &raw, default)),
        };

        let endpoint = text(self.endpoint, "SIDESEAT_ENDPOINT")
            .or_else(|| text(None, "OTEL_EXPORTER_OTLP_ENDPOINT"))
            .unwrap_or_else(|| DEFAULT_ENDPOINT.to_string());
        let project = text(self.project, "SIDESEAT_PROJECT_ID")
            .map(|project| project.trim().to_string())
            .unwrap_or_else(|| DEFAULT_PROJECT.to_string());
        let mut resource_attributes = BTreeMap::new();
        for attribute in self.resource_attributes {
            resource_attributes.insert(attribute.key.to_string(), attribute);
        }

        let settings = Settings {
            otlp_base: otlp_base(&endpoint, &project)?,
            endpoint: endpoint.trim().trim_end_matches('/').to_string(),
            project,
            api_key: text(self.api_key, "SIDESEAT_API_KEY"),
            headers: parse_headers(env("OTEL_EXPORTER_OTLP_HEADERS").as_deref()),
            service_name: text(self.service_name, "OTEL_SERVICE_NAME")
                .unwrap_or_else(|| DEFAULT_SERVICE_NAME.to_string()),
            service_version: text(self.service_version, "OTEL_SERVICE_VERSION")
                .unwrap_or_else(|| crate::VERSION.to_string()),
            capture_content: flag(self.capture_content, "SIDESEAT_CAPTURE_CONTENT", true)?,
            disabled: flag(self.disabled, "SIDESEAT_DISABLED", false)?,
            debug: flag(self.debug, "SIDESEAT_DEBUG", false)?,
            export: self.export.unwrap_or(true),
            logs: self.logs.unwrap_or(true),
            resource_attributes: resource_attributes.into_values().collect(),
        };
        Ok((settings, self.span_processors))
    }
}

impl fmt::Debug for Options {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Options")
            .field("endpoint", &self.endpoint)
            .field("project", &self.project)
            .field("api_key", &self.api_key.as_ref().map(|_| "<redacted>"))
            .field("service_name", &self.service_name)
            .field("span_processors", &self.span_processors.len())
            .finish_non_exhaustive()
    }
}

/// Resolved, immutable settings.
#[derive(Clone, PartialEq, Eq)]
pub struct Settings {
    pub endpoint: String,
    pub project: String,
    pub api_key: Option<String>,
    pub service_name: String,
    pub service_version: String,
    pub capture_content: bool,
    pub disabled: bool,
    pub debug: bool,
    pub export: bool,
    pub logs: bool,
    pub resource_attributes: Vec<KeyValue>,
    otlp_base: String,
    headers: BTreeMap<String, String>,
}

impl Settings {
    /// Where `signal` (`traces`, `logs`, or `metrics`) is exported.
    pub fn signal_endpoint(&self, signal: &str) -> String {
        format!("{}/v1/{signal}", self.otlp_base)
    }

    /// `OTEL_EXPORTER_OTLP_HEADERS` plus the API key, which wins on conflict.
    pub(crate) fn export_headers(&self) -> std::collections::HashMap<String, String> {
        let mut headers: std::collections::HashMap<_, _> = self
            .headers
            .iter()
            .filter(|(name, _)| {
                !name.eq_ignore_ascii_case("authorization") || self.api_key.is_none()
            })
            .map(|(name, value)| (name.clone(), value.clone()))
            .collect();
        if let Some(key) = &self.api_key {
            headers.insert("Authorization".into(), format!("Bearer {key}"));
        }
        headers
    }
}

impl fmt::Debug for Settings {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Settings")
            .field("endpoint", &self.endpoint)
            .field("project", &self.project)
            .field("api_key", &self.api_key.as_ref().map(|_| "<redacted>"))
            .field("service_name", &self.service_name)
            .field("service_version", &self.service_version)
            .field("capture_content", &self.capture_content)
            .field("disabled", &self.disabled)
            .field("export", &self.export)
            .field("logs", &self.logs)
            .field("traces", &self.signal_endpoint("traces"))
            .finish_non_exhaustive()
    }
}

/// An endpoint without a path is a SideSeat server; one with a path is already an OTLP base.
fn otlp_base(endpoint: &str, project: &str) -> Result<String, Error> {
    let trimmed = endpoint.trim().trim_end_matches('/');
    let invalid =
        || Error::Configuration(format!("endpoint must be an http(s) URL, got {endpoint:?}"));
    let url = Url::parse(trimmed).map_err(|_| invalid())?;
    if !matches!(url.scheme(), "http" | "https") || url.host_str().is_none() {
        return Err(invalid());
    }
    if url.path().trim_end_matches('/').is_empty() {
        let project = percent_encoding::utf8_percent_encode(project, COMPONENT);
        Ok(format!("{trimmed}/otel/{project}"))
    } else {
        Ok(trimmed.to_string())
    }
}

fn parse_flag(name: &str, raw: &str, default: bool) -> Result<bool, Error> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "" => Ok(default),
        "1" | "true" | "yes" => Ok(true),
        "0" | "false" | "no" => Ok(false),
        _ => Err(Error::Configuration(format!(
            "{name} must be one of 1/0, true/false, yes/no; got {raw:?}"
        ))),
    }
}

/// `key=value` pairs separated by commas, percent-decoded, as the OTLP exporter specification
/// defines them.
fn parse_headers(raw: Option<&str>) -> BTreeMap<String, String> {
    let decode = |text: &str| {
        percent_encoding::percent_decode_str(text.trim())
            .decode_utf8_lossy()
            .into_owned()
    };
    raw.unwrap_or_default()
        .split(',')
        .filter_map(|pair| pair.split_once('='))
        .filter(|(name, _)| !name.trim().is_empty())
        .map(|(name, value)| (decode(name), decode(value)))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn resolve(options: Options, env: &[(&str, &str)]) -> Result<Settings, Error> {
        let env: BTreeMap<String, String> = env
            .iter()
            .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
            .collect();
        options
            .resolve_with(|name| env.get(name).cloned())
            .map(|(settings, _)| settings)
    }

    #[test]
    fn defaults_point_at_a_local_server_and_capture_content() {
        let settings = resolve(Options::new(), &[]).unwrap();

        assert_eq!(
            settings.signal_endpoint("traces"),
            "http://127.0.0.1:5388/otel/default/v1/traces"
        );
        assert_eq!(settings.service_name, "sideseat-app");
        assert_eq!(settings.service_version, crate::VERSION);
        assert!(settings.capture_content);
        assert!(!settings.disabled);
    }

    #[test]
    fn explicit_options_win_over_the_environment() {
        let env = [
            ("SIDESEAT_ENDPOINT", "http://env:1"),
            ("SIDESEAT_PROJECT_ID", "env-project"),
            ("SIDESEAT_CAPTURE_CONTENT", "false"),
        ];

        let settings = resolve(
            Options::new()
                .endpoint("https://explicit.example")
                .project("mine")
                .capture_content(true),
            &env,
        )
        .unwrap();

        assert_eq!(
            settings.signal_endpoint("logs"),
            "https://explicit.example/otel/mine/v1/logs"
        );
        assert!(settings.capture_content);
    }

    #[test]
    fn the_sideseat_endpoint_wins_over_the_generic_otlp_endpoint() {
        let both = [
            ("SIDESEAT_ENDPOINT", "http://sideseat:5388"),
            ("OTEL_EXPORTER_OTLP_ENDPOINT", "http://collector:4318"),
        ];
        let generic = [("OTEL_EXPORTER_OTLP_ENDPOINT", "http://collector:4318/")];

        assert_eq!(
            resolve(Options::new(), &both).unwrap().endpoint,
            "http://sideseat:5388"
        );
        assert_eq!(
            resolve(Options::new(), &generic)
                .unwrap()
                .signal_endpoint("traces"),
            "http://collector:4318/otel/default/v1/traces"
        );
    }

    #[test]
    fn an_endpoint_with_a_path_is_already_an_otlp_base() {
        let settings = resolve(Options::new().endpoint("http://gw:4318/otel/team/"), &[]).unwrap();

        assert_eq!(
            settings.signal_endpoint("metrics"),
            "http://gw:4318/otel/team/v1/metrics"
        );
    }

    #[test]
    fn the_project_is_encoded_into_the_path() {
        let settings = resolve(Options::new().project("a b/c"), &[]).unwrap();

        assert_eq!(
            settings.signal_endpoint("traces"),
            "http://127.0.0.1:5388/otel/a%20b%2Fc/v1/traces"
        );
    }

    #[test]
    fn invalid_endpoints_and_flags_are_errors() {
        for endpoint in ["ftp://host", "not a url", "http://"] {
            assert!(
                matches!(
                    resolve(Options::new().endpoint(endpoint), &[]),
                    Err(Error::Configuration(_))
                ),
                "{endpoint}"
            );
        }
        assert!(matches!(
            resolve(Options::new(), &[("SIDESEAT_DISABLED", "maybe")]),
            Err(Error::Configuration(message)) if message.contains("SIDESEAT_DISABLED")
        ));
    }

    #[test]
    fn flags_accept_every_documented_spelling_in_any_case() {
        for (raw, expected) in [
            ("1", true),
            ("TRUE", true),
            ("Yes", true),
            ("0", false),
            ("False", false),
            ("NO", false),
        ] {
            let settings = resolve(Options::new(), &[("SIDESEAT_DISABLED", raw)]).unwrap();
            assert_eq!(settings.disabled, expected, "{raw}");
        }
    }

    #[test]
    fn otlp_headers_are_kept_and_the_api_key_wins() {
        let env = [(
            "OTEL_EXPORTER_OTLP_HEADERS",
            "x-team=ai%20platform, Authorization=Basic abc,=ignored",
        )];

        let with_key = resolve(Options::new().api_key("k"), &env)
            .unwrap()
            .export_headers();
        let without_key = resolve(Options::new(), &env).unwrap().export_headers();

        assert_eq!(
            with_key.get("x-team").map(String::as_str),
            Some("ai platform")
        );
        assert_eq!(
            with_key.get("Authorization").map(String::as_str),
            Some("Bearer k")
        );
        assert_eq!(with_key.len(), 2);
        assert_eq!(
            without_key.get("Authorization").map(String::as_str),
            Some("Basic abc")
        );
    }

    #[test]
    fn settings_debug_output_redacts_the_api_key() {
        let settings = resolve(Options::new().api_key("secret-key"), &[]).unwrap();

        assert!(!format!("{settings:?}").contains("secret-key"));
    }
}
