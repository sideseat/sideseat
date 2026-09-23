//! Provider credential management.

pub mod catalog;
mod service;

pub use service::{
    CredentialConnectionTester, CredentialError, CredentialService, CredentialSource,
    ResolvedCredential, TestResult,
};
