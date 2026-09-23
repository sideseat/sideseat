//! Blob-storage adapters.

mod filesystem;
mod s3;

pub use filesystem::FilesystemStorage;
pub use s3::S3Storage;
