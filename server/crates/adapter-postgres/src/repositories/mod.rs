//! PostgreSQL repositories.
//!
//! Shared rows belong to `sideseat_ports::types`; this module contains PostgreSQL implementations.

pub mod api_key;
pub mod auth_method;
pub mod body;
pub mod credential_permissions;
pub mod credentials;
pub mod favorite;
pub mod file;
pub mod governance;
pub mod journal;
pub mod membership;
pub mod organization;
pub mod project;
pub mod restore;
pub mod staging;
pub mod user;
