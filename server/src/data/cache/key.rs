//! Type-safe cache key builder with versioning

use crate::core::constants::CACHE_KEY_VERSION;

/// Type-safe cache key builder
///
/// All keys are prefixed with a version (e.g., "v1:") to allow
/// invalidating all cached data on schema changes.
pub struct CacheKey;

impl CacheKey {
    // =========================================================================
    // Users
    // =========================================================================

    /// Cache key for user by ID
    pub fn user(id: &str) -> String {
        format!("{}:user:{}", CACHE_KEY_VERSION, id)
    }

    /// Cache key for user by email
    pub fn user_by_email(email: &str) -> String {
        format!("{}:user:email:{}", CACHE_KEY_VERSION, email.to_lowercase())
    }

    /// Cache key for negative user lookup by ID (not found)
    pub fn user_negative(id: &str) -> String {
        format!("{}:user:neg:{}", CACHE_KEY_VERSION, id)
    }

    /// Cache key for negative user lookup by email (not found)
    pub fn user_by_email_negative(email: &str) -> String {
        format!(
            "{}:user:email:neg:{}",
            CACHE_KEY_VERSION,
            email.to_lowercase()
        )
    }

    // =========================================================================
    // Organizations
    // =========================================================================

    /// Cache key for organization by ID
    pub fn organization(id: &str) -> String {
        format!("{}:org:{}", CACHE_KEY_VERSION, id)
    }

    /// Cache key for organization by slug
    pub fn org_by_slug(slug: &str) -> String {
        format!("{}:org:slug:{}", CACHE_KEY_VERSION, slug)
    }

    /// Cache key for organizations list for a user
    pub fn orgs_for_user(user_id: &str) -> String {
        format!("{}:orgs:user:{}", CACHE_KEY_VERSION, user_id)
    }

    // =========================================================================
    // Projects
    // =========================================================================

    /// Cache key for project by ID
    pub fn project(id: &str) -> String {
        format!("{}:project:{}", CACHE_KEY_VERSION, id)
    }

    /// Cache key for projects list for a user
    pub fn projects_for_user(user_id: &str) -> String {
        format!("{}:projects:user:{}", CACHE_KEY_VERSION, user_id)
    }

    /// Cache key for projects list for an organization
    pub fn projects_for_org(org_id: &str) -> String {
        format!("{}:projects:org:{}", CACHE_KEY_VERSION, org_id)
    }

    // =========================================================================
    // Memberships
    // =========================================================================

    /// Cache key for membership (org + user)
    pub fn membership(org_id: &str, user_id: &str) -> String {
        format!("{}:membership:{}:{}", CACHE_KEY_VERSION, org_id, user_id)
    }

    /// Cache key for user org membership boolean (for auth checks)
    pub fn user_org_member(user_id: &str, org_id: &str) -> String {
        format!("{}:member:{}:{}", CACHE_KEY_VERSION, user_id, org_id)
    }

    /// Cache key for project's organization ID (for auth checks)
    pub fn project_org(project_id: &str) -> String {
        format!("{}:projorg:{}", CACHE_KEY_VERSION, project_id)
    }

    // =========================================================================
    // Auth Methods
    // =========================================================================

    /// Cache key for OAuth auth method
    pub fn auth_oauth(provider: &str, provider_id: &str) -> String {
        format!(
            "{}:auth:oauth:{}:{}",
            CACHE_KEY_VERSION, provider, provider_id
        )
    }

    /// Cache key for auth methods list for a user
    pub fn auth_methods_for_user(user_id: &str) -> String {
        format!("{}:auth:user:{}", CACHE_KEY_VERSION, user_id)
    }

    // =========================================================================
    // Stats (DuckDB)
    // =========================================================================

    /// Cache key for project stats
    ///
    /// Uses an 8-char hash of timezone to keep keys short
    pub fn stats(project_id: &str, from: i64, to: i64, tz: &str) -> String {
        let tz_hash = &format!("{:x}", md5::compute(tz))[..8];
        format!(
            "{}:stats:{}:{}:{}:{}",
            CACHE_KEY_VERSION, project_id, from, to, tz_hash
        )
    }

    // =========================================================================
    // API Keys
    // =========================================================================

    /// Cache key for API key by hash (for validation lookups)
    pub fn api_key_by_hash(hash: &str) -> String {
        format!("{}:apikey:{}", CACHE_KEY_VERSION, hash)
    }

    /// Cache key for negative API key lookup by hash (not found)
    pub fn api_key_negative(hash: &str) -> String {
        format!("{}:apikey:neg:{}", CACHE_KEY_VERSION, hash)
    }

    /// Cache key for API keys list for an organization
    pub fn api_keys_for_org(org_id: &str) -> String {
        format!("{}:apikeys:org:{}", CACHE_KEY_VERSION, org_id)
    }

    // =========================================================================
    // File Quota
    // =========================================================================

    /// Cache key for project file storage bytes (quota check)
    pub fn file_quota(project_id: &str) -> String {
        format!("{}:filequota:{}", CACHE_KEY_VERSION, project_id)
    }

    // =========================================================================
    // Rate Limiting
    // =========================================================================

    /// Cache key for rate limit counter
    ///
    /// Note: Rate limit keys are NOT versioned (counter semantics don't change)
    ///
    /// The identifier is used directly without escaping. Callers should ensure
    /// identifiers don't contain characters that could cause key collisions
    /// (e.g., bucket names shouldn't contain `:` and identifiers are typically
    /// IP addresses or project IDs which are safe).
    pub fn rate_limit(bucket: &str, identifier: &str) -> String {
        format!("rl:{}:{}", bucket, identifier)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_user_keys() {
        assert_eq!(CacheKey::user("u123"), "v1:user:u123");
        assert_eq!(
            CacheKey::user_by_email("Test@Example.COM"),
            "v1:user:email:test@example.com"
        );
        assert_eq!(CacheKey::user_negative("u123"), "v1:user:neg:u123");
    }

    #[test]
    fn test_org_keys() {
        assert_eq!(CacheKey::organization("org1"), "v1:org:org1");
        assert_eq!(CacheKey::org_by_slug("my-org"), "v1:org:slug:my-org");
        assert_eq!(CacheKey::orgs_for_user("u1"), "v1:orgs:user:u1");
    }

    #[test]
    fn test_project_keys() {
        assert_eq!(CacheKey::project("p1"), "v1:project:p1");
        assert_eq!(CacheKey::projects_for_user("u1"), "v1:projects:user:u1");
        assert_eq!(CacheKey::projects_for_org("o1"), "v1:projects:org:o1");
    }

    #[test]
    fn test_membership_key() {
        assert_eq!(
            CacheKey::membership("org1", "user1"),
            "v1:membership:org1:user1"
        );
    }

    #[test]
    fn test_auth_keys() {
        assert_eq!(
            CacheKey::auth_oauth("google", "12345"),
            "v1:auth:oauth:google:12345"
        );
        assert_eq!(CacheKey::auth_methods_for_user("u1"), "v1:auth:user:u1");
    }

    #[test]
    fn test_stats_key() {
        let key = CacheKey::stats("proj1", 1000, 2000, "America/New_York");
        assert!(key.starts_with("v1:stats:proj1:1000:2000:"));
        assert_eq!(key.len(), "v1:stats:proj1:1000:2000:".len() + 8);
    }

    #[test]
    fn test_rate_limit_key() {
        // Rate limit keys are NOT versioned
        assert_eq!(
            CacheKey::rate_limit("api", "192.168.1.1"),
            "rl:api:192.168.1.1"
        );
    }

    #[test]
    fn test_api_key_keys() {
        assert_eq!(CacheKey::api_key_by_hash("abc123"), "v1:apikey:abc123");
        assert_eq!(CacheKey::api_key_negative("abc123"), "v1:apikey:neg:abc123");
        assert_eq!(CacheKey::api_keys_for_org("org1"), "v1:apikeys:org:org1");
    }
}
