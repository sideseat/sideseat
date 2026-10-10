//! Blob-storage adapters.

// A map's iteration order differs from one process to the next: where it reaches stored or answered bytes, a
// hash or the order of a write, iterate in order; elsewhere say why order cannot matter.
#![deny(clippy::iter_over_hash_type)]

mod filesystem;
mod s3;

pub use filesystem::FilesystemStorage;
pub use s3::S3Storage;
