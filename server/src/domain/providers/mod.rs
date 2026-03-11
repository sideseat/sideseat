//! Provider credential management domain.
//!
//! Centralizes LLM provider credential operations: storage, secret management,
//! environment variable scanning, and connection testing.

pub mod catalog;
mod service;
mod test_connection;

pub use service::{
    CredentialError, CredentialService, CredentialSource, ResolvedCredential, TestResult,
};
