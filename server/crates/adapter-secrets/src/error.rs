use sideseat_core::utils::crypto::HexDecodeError;
use thiserror::Error;

/// Everything that can go wrong reading, writing or provisioning a secret.
///
/// One type for the whole crate rather than one per backend, because the manager's own operations -
/// provisioning a root secret, refusing to start - fail *through* a backend and have to name both.
/// Several variants therefore carry a `Box<SecretError>` cause: that is the chain `anyhow`'s
/// `.context()` used to build, kept as a source so `{:#}` and `Error::source` still walk it.
#[derive(Error, Debug)]
pub enum SecretError {
    #[error("Secret not found: {0}")]
    NotFound(String),

    #[error("Secret backend error ({backend}): {message}")]
    Backend {
        backend: &'static str,
        message: String,
    },

    #[error("Secret backend is read-only ({backend})")]
    ReadOnly { backend: &'static str },

    /// A read-only backend missing a secret SideSeat is not allowed to create for itself.
    #[error(
        "Secret backend '{backend}' is read-only. Required secrets missing: {missing}. \
         Pre-configure these before starting the server."
    )]
    ReadOnlyMissingSecrets {
        backend: &'static str,
        missing: String,
    },

    #[error("Secret serialization error: {0}")]
    Serialization(String),

    #[error("Secret configuration error: {0}")]
    Config(String),

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("HTTP error: {0}")]
    Http(#[from] reqwest::Error),

    // -- The file-backed vault --
    #[error("Failed to load secrets file: {source}")]
    LoadSecretsFile {
        #[source]
        source: std::io::Error,
    },

    #[error("Failed to serialize vault after migration")]
    SerializeMigratedVault {
        #[source]
        source: serde_json::Error,
    },

    #[error("Failed to save migrated vault")]
    SaveMigratedVault {
        #[source]
        source: Box<SecretError>,
    },

    // -- The OS credential store --
    #[error("Failed to create keychain entry: {source}")]
    KeychainEntry {
        #[source]
        source: keyring::Error,
    },

    #[error("Failed to load from keychain: {source}")]
    KeychainLoad {
        #[source]
        source: keyring::Error,
    },

    #[error("Failed to save to keychain: {source}")]
    KeychainSave {
        #[source]
        source: keyring::Error,
    },

    #[error("Keychain reader thread died")]
    KeychainReaderGone {
        #[source]
        source: tokio::sync::oneshot::error::RecvError,
    },

    #[error("Keychain task failed")]
    KeychainTask {
        #[source]
        source: tokio::task::JoinError,
    },

    #[error("Failed to serialize vault")]
    SerializeVault {
        #[source]
        source: serde_json::Error,
    },

    /// An authorization dialog nobody can answer. The remedy is in the message because the symptom -
    /// a process that has simply stopped - gives no clue where to look.
    #[error(
        "the OS credential store did not respond within {timeout_secs}s. An authorization window \
         is probably open and unanswered - look behind the terminal, and approve it \
         with Always Allow so it stops asking. A background process, an SSH session \
         or CI has no way to answer it at all; there, configure the file backend \
         instead: `secrets: {{ backend: \"file\" }}` in sideseat.json."
    )]
    KeychainTimeout { timeout_secs: u64 },

    // -- Root secrets, which the manager provisions --
    /// The backend answered neither "here it is" nor "there is none", so a replacement cannot be
    /// generated: the live secret is probably still there, merely unreadable.
    #[error(
        "could not read the {label} from the {backend} secrets backend: {source}. Refusing to \
         start: generating a replacement would mean {consequence}. Restore access to the backend, \
         or set the secret explicitly."
    )]
    RootSecretUnreadable {
        label: &'static str,
        backend: &'static str,
        consequence: &'static str,
        #[source]
        source: Box<SecretError>,
    },

    #[error(
        "could not tell whether the {label} is already stored in the {backend} secrets backend; \
         refusing to overwrite a secret that may exist"
    )]
    RootSecretPresenceUnknown {
        label: &'static str,
        backend: &'static str,
        #[source]
        source: Box<SecretError>,
    },

    #[error("secret {key} is not valid hex")]
    RootSecretNotHex {
        key: &'static str,
        #[source]
        source: HexDecodeError,
    },

    #[error("secret {key} is not {expected} bytes ({stored} stored)")]
    RootSecretWrongLength {
        key: &'static str,
        expected: usize,
        stored: usize,
    },
}

impl SecretError {
    pub fn backend(backend: &'static str, msg: impl Into<String>) -> Self {
        Self::Backend {
            backend,
            message: msg.into(),
        }
    }
}
