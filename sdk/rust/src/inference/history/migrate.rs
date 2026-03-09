use super::error::HistoryError;

pub const CURRENT_SCHEMA_VERSION: u32 = 1;

pub fn check_schema_version(found: u32) -> Result<(), HistoryError> {
    if found > CURRENT_SCHEMA_VERSION {
        return Err(HistoryError::UnsupportedVersion {
            found,
            max: CURRENT_SCHEMA_VERSION,
        });
    }
    Ok(())
}
