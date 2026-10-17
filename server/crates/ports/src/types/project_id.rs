use std::ffi::OsStr;
use std::fmt;
use std::ops::Deref;
use std::path::Path;

use serde::{Deserialize, Serialize};

/// A tenant identifier at a port boundary.
///
/// Keeping this distinct from other caller-supplied strings prevents a trace,
/// session, organization, or user id from being passed where tenant scope is
/// required. The transparent representation preserves the existing wire
/// format.
#[derive(Clone, Debug, Default, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(transparent)]
pub struct ProjectId(String);

impl ProjectId {
    #[must_use]
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    /// Whether `id` has a project id's form: 1 to 64 characters, each alphanumeric, `-` or `_`. Every API that
    /// takes a project id refuses anything else, and a store that writes the id into a statement as a literal
    /// refuses it too, so no quote, backslash or control character ever reaches one.
    #[must_use]
    pub fn is_well_formed(id: &str) -> bool {
        !id.is_empty()
            && id.len() <= 64
            && id
                .chars()
                .all(|c| c.is_alphanumeric() || c == '-' || c == '_')
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    #[must_use]
    pub fn into_inner(self) -> String {
        self.0
    }
}

impl AsRef<str> for ProjectId {
    fn as_ref(&self) -> &str {
        self.as_str()
    }
}

impl AsRef<OsStr> for ProjectId {
    fn as_ref(&self) -> &OsStr {
        OsStr::new(self.as_str())
    }
}

impl AsRef<Path> for ProjectId {
    fn as_ref(&self) -> &Path {
        Path::new(self.as_str())
    }
}

impl Deref for ProjectId {
    type Target = str;

    fn deref(&self) -> &Self::Target {
        self.as_str()
    }
}

impl fmt::Display for ProjectId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl From<String> for ProjectId {
    fn from(value: String) -> Self {
        Self(value)
    }
}

impl From<&str> for ProjectId {
    fn from(value: &str) -> Self {
        Self(value.to_owned())
    }
}

impl From<&String> for ProjectId {
    fn from(value: &String) -> Self {
        Self(value.clone())
    }
}

impl From<ProjectId> for String {
    fn from(value: ProjectId) -> Self {
        value.into_inner()
    }
}

impl PartialEq<str> for ProjectId {
    fn eq(&self, other: &str) -> bool {
        self.as_str() == other
    }
}

impl PartialEq<&str> for ProjectId {
    fn eq(&self, other: &&str) -> bool {
        self.as_str() == *other
    }
}

#[cfg(test)]
mod tests {
    use super::ProjectId;

    #[test]
    fn serde_representation_stays_a_plain_string() {
        let project_id = ProjectId::from("tenant-a");

        assert_eq!(serde_json::to_string(&project_id).unwrap(), r#""tenant-a""#);
        assert_eq!(
            serde_json::from_str::<ProjectId>(r#""tenant-a""#).unwrap(),
            project_id
        );
    }

    #[test]
    fn owned_and_borrowed_inputs_have_the_same_identity() {
        let owned = "tenant-a".to_owned();

        assert_eq!(ProjectId::from(owned.clone()), ProjectId::from(&owned));
        assert_eq!(ProjectId::from(owned.as_str()), "tenant-a");
    }
}
