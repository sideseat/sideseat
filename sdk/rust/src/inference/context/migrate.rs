use super::error::CmError;

pub const CURRENT_SCHEMA_VERSION: u32 = 1;

pub fn check_schema_version(found: u32) -> Result<(), CmError> {
    if found > CURRENT_SCHEMA_VERSION {
        return Err(CmError::UnsupportedVersion {
            found,
            max: CURRENT_SCHEMA_VERSION,
        });
    }
    Ok(())
}
