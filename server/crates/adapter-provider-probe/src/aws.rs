//! Amazon Bedrock: AWS credentials in four forms, checked by listing foundation models.
//!
//! An identity may be allowed to invoke models without being allowed to list them, so a refused
//! listing falls back to a one-token Converse request. A model the account cannot use still proves
//! the credential.

use aws_config::sts::AssumeRoleProvider;
use aws_config::{BehaviorVersion, Region, SdkConfig};
use aws_credential_types::Credentials;
use aws_sdk_bedrock::config::http::HttpResponse;
use aws_sdk_bedrock::error::{DisplayErrorContext, ProvideErrorMetadata, SdkError};
use aws_sdk_bedrockruntime::types::{
    ContentBlock, ConversationRole, InferenceConfiguration, Message,
};
use serde_json::Value;

use crate::{Endpoints, ProbeError, ProbeTarget};

const MODEL: &str = "global.anthropic.claude-haiku-4-5-20251001-v1:0";
const SESSION_NAME: &str = "sideseat";

pub(crate) async fn probe(
    target: ProbeTarget<'_>,
    endpoints: &Endpoints,
) -> Result<Option<String>, ProbeError> {
    let region = target.option("region").unwrap_or("us-east-1").to_string();
    let (control, runtime) = match target.option("auth_mode").unwrap_or("bearer") {
        "access_keys" | "iam_role" | "iam_ambient" => {
            let config = sdk_config(target, &region, endpoints).await?;
            (
                aws_sdk_bedrock::Client::new(&config),
                aws_sdk_bedrockruntime::Client::new(&config),
            )
        }
        _ => bearer_clients(target.secret(), &region, endpoints),
    };

    let listing_error = match control.list_foundation_models().send().await {
        Ok(output) => {
            return Ok(output
                .model_summaries()
                .first()
                .map(|model| model.model_id().to_string()));
        }
        Err(error) => sdk_error(error),
    };
    if !matches!(&listing_error, Failure::Service { code, .. } if code == "AccessDeniedException") {
        return Err(listing_error.into());
    }

    let message = Message::builder()
        .role(ConversationRole::User)
        .content(ContentBlock::Text("Hello".into()))
        .build()
        .map_err(|e| ProbeError::Configuration(e.to_string()))?;
    match runtime
        .converse()
        .model_id(MODEL)
        .messages(message)
        .inference_config(InferenceConfiguration::builder().max_tokens(1).build())
        .send()
        .await
    {
        Ok(_) => Ok(Some(MODEL.to_string())),
        Err(error) => match sdk_error(error) {
            Failure::Service { code, .. } if code == "ResourceNotFoundException" => Ok(None),
            Failure::Service { code, message, .. }
                if code == "ValidationException" && crate::http::mentions_model(&message) =>
            {
                Ok(None)
            }
            failure => Err(failure.into()),
        },
    }
}

/// An AWS SDK error reduced to what the probe reports.
enum Failure {
    Service {
        status: u16,
        code: String,
        message: String,
    },
    Transport(String),
}

fn sdk_error<E>(error: SdkError<E, HttpResponse>) -> Failure
where
    E: ProvideErrorMetadata + std::error::Error + 'static,
{
    match error {
        SdkError::ServiceError(context) => {
            let status = context.raw().status().as_u16();
            let error = context.into_err();
            Failure::Service {
                status,
                code: error.code().unwrap_or("UnknownError").to_string(),
                message: error.message().unwrap_or_default().to_string(),
            }
        }
        other => Failure::Transport(DisplayErrorContext(&other).to_string()),
    }
}

impl From<Failure> for ProbeError {
    fn from(failure: Failure) -> Self {
        match failure {
            Failure::Transport(message) => ProbeError::Unreachable(message),
            Failure::Service {
                status,
                code,
                message,
            } => {
                let message = if message.is_empty() {
                    code
                } else {
                    format!("{code}: {message}")
                };
                crate::http::classify(
                    reqwest::StatusCode::from_u16(status)
                        .unwrap_or(reqwest::StatusCode::BAD_GATEWAY),
                    message,
                )
            }
        }
    }
}

async fn sdk_config(
    target: ProbeTarget<'_>,
    region: &str,
    endpoints: &Endpoints,
) -> Result<SdkConfig, ProbeError> {
    let region = Region::new(region.to_string());
    let mut loader = aws_config::defaults(BehaviorVersion::latest()).region(region.clone());
    if let Some(origin) = &endpoints.origin {
        loader = loader.endpoint_url(origin);
    }
    let static_credentials = match target.option("auth_mode") {
        Some("access_keys") => Some(static_credentials(target.secret())?),
        _ => None,
    };

    if let Some(role_arn) = target.option("role_arn") {
        let mut role = AssumeRoleProvider::builder(role_arn)
            .region(region)
            .session_name(SESSION_NAME);
        if let Some(external_id) = target.option("external_id") {
            role = role.external_id(external_id);
        }
        let role = match static_credentials {
            Some(base) => role.build_from_provider(base).await,
            None => role.build().await,
        };
        loader = loader.credentials_provider(role);
    } else if let Some(credentials) = static_credentials {
        loader = loader.credentials_provider(credentials);
    }
    Ok(loader.load().await)
}

/// Access keys arrive as a JSON document: `access_key_id`, `secret_access_key`, and optionally
/// `session_token`.
fn static_credentials(secret: &str) -> Result<Credentials, ProbeError> {
    let document: Value = serde_json::from_str(secret)
        .map_err(|e| ProbeError::Configuration(format!("invalid Bedrock credentials JSON: {e}")))?;
    let field = |name: &str| {
        document
            .get(name)
            .and_then(Value::as_str)
            .map(ToString::to_string)
    };
    let access_key_id = field("access_key_id")
        .ok_or_else(|| ProbeError::Configuration("missing access_key_id".into()))?;
    let secret_access_key = field("secret_access_key")
        .ok_or_else(|| ProbeError::Configuration("missing secret_access_key".into()))?;
    Ok(Credentials::new(
        access_key_id,
        secret_access_key,
        field("session_token"),
        None,
        SESSION_NAME,
    ))
}

/// A Bedrock API key is a bearer token; it signs nothing, so it needs no credential chain.
fn bearer_clients(
    api_key: &str,
    region: &str,
    endpoints: &Endpoints,
) -> (aws_sdk_bedrock::Client, aws_sdk_bedrockruntime::Client) {
    let mut control = aws_sdk_bedrock::config::Builder::new()
        .behavior_version(BehaviorVersion::latest())
        .region(Region::new(region.to_string()))
        .bearer_token(aws_sdk_bedrock::config::Token::new(api_key, None));
    let mut runtime = aws_sdk_bedrockruntime::config::Builder::new()
        .behavior_version(BehaviorVersion::latest())
        .region(Region::new(region.to_string()))
        .bearer_token(aws_sdk_bedrockruntime::config::Token::new(api_key, None));
    if let Some(origin) = &endpoints.origin {
        control = control.endpoint_url(origin);
        runtime = runtime.endpoint_url(origin);
    }
    (
        aws_sdk_bedrock::Client::from_conf(control.build()),
        aws_sdk_bedrockruntime::Client::from_conf(runtime.build()),
    )
}
