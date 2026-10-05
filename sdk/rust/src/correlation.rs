//! Session and user correlation.
//!
//! The values travel in the OpenTelemetry context under a private key rather than W3C baggage:
//! HTTP client instrumentation injects baggage into every outgoing request, which would send
//! end-user identifiers to model providers. A private key follows the same paths inside the
//! process - across `.await` points with [`FutureExt::with_context`] - and never leaves it.

use std::future::Future;
use std::time::Duration;

use opentelemetry::trace::FutureExt as _;
use opentelemetry::{Context, ContextGuard, KeyValue};
use opentelemetry_sdk::error::OTelSdkResult;
use opentelemetry_sdk::trace::{Span, SpanData, SpanProcessor};

use crate::Error;

/// The session and user a span belongs to.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Correlation {
    pub session_id: Option<String>,
    pub user_id: Option<String>,
}

impl Correlation {
    pub fn of(context: &Context) -> Self {
        context.get::<Self>().cloned().unwrap_or_default()
    }

    /// `context` with these values layered over the ones it already carries.
    pub fn apply(self, context: &Context) -> Context {
        let inherited = Self::of(context);
        context.with_value(Self {
            session_id: self.session_id.or(inherited.session_id),
            user_id: self.user_id.or(inherited.user_id),
        })
    }
}

/// A scope whose spans - including spans a framework creates - belong to one session and,
/// optionally, one user. A nested session overrides the outer one for its duration.
///
/// ```no_run
/// # async fn agent_turn() {}
/// # async fn example() -> Result<(), sideseat::Error> {
/// let conversation = sideseat::Session::new("conversation-42")?.user("user-7")?;
/// conversation.scope(agent_turn()).await;
///
/// let _entered = conversation.enter(); // synchronous code on this thread
/// # Ok(())
/// # }
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
#[must_use]
pub struct Session {
    correlation: Correlation,
}

impl Session {
    /// A session with this id.
    ///
    /// # Errors
    ///
    /// [`Error::EmptyId`] if `id` is empty: an empty session id would silently merge unrelated
    /// conversations.
    pub fn new(id: impl Into<String>) -> Result<Self, Error> {
        Ok(Self {
            correlation: Correlation {
                session_id: Some(non_empty("session id", id.into())?),
                user_id: None,
            },
        })
    }

    /// The user the session belongs to.
    ///
    /// # Errors
    ///
    /// [`Error::EmptyId`] if `id` is empty.
    pub fn user(mut self, id: impl Into<String>) -> Result<Self, Error> {
        self.correlation.user_id = Some(non_empty("user id", id.into())?);
        Ok(self)
    }

    /// Runs `future` inside the session. The context travels with the future, so the session
    /// holds wherever the future is polled.
    pub fn scope<F: Future>(&self, future: F) -> impl Future<Output = F::Output> {
        future.with_context(self.context(&Context::current()))
    }

    /// Makes the session current on this thread until the guard drops. For async code, use
    /// [`scope`](Self::scope): a guard held across `.await` does not follow the task.
    pub fn enter(&self) -> ContextGuard {
        self.context(&Context::current()).attach()
    }

    pub(crate) fn context(&self, parent: &Context) -> Context {
        self.correlation.clone().apply(parent)
    }

    pub(crate) fn correlation(&self) -> &Correlation {
        &self.correlation
    }
}

fn non_empty(what: &'static str, value: String) -> Result<String, Error> {
    if value.is_empty() {
        Err(Error::EmptyId(what))
    } else {
        Ok(value)
    }
}

/// Stamps `session.id` and `user.id` on every span started inside a session. It is registered
/// first, so the attributes exist before any other processor or exporter reads the span.
#[derive(Debug, Default)]
pub(crate) struct CorrelationProcessor;

impl SpanProcessor for CorrelationProcessor {
    fn on_start(&self, span: &mut Span, parent: &Context) {
        use opentelemetry::trace::Span as _;
        let Correlation {
            session_id,
            user_id,
        } = Correlation::of(parent);
        if let Some(id) = session_id {
            span.set_attribute(KeyValue::new("session.id", id));
        }
        if let Some(id) = user_id {
            span.set_attribute(KeyValue::new("user.id", id));
        }
    }

    fn on_end(&self, _span: SpanData) {}

    fn force_flush(&self) -> OTelSdkResult {
        Ok(())
    }

    fn shutdown_with_timeout(&self, _timeout: Duration) -> OTelSdkResult {
        Ok(())
    }
}
