//! Blob-storage adapters.

pub mod filesystem;
pub mod s3;

pub use filesystem::FilesystemStorage;
pub use s3::S3Storage;
