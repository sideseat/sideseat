//! EAV attribute storage and caching

use dashmap::DashMap;
use serde::{Deserialize, Serialize};
use sqlx::{Sqlite, SqlitePool, Transaction};
use std::sync::Arc;

use crate::otel::error::OtelError;

/// Attribute key information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AttributeKey {
    pub id: i64,
    pub key_name: String,
    pub key_type: String,
    pub entity_type: String,
    pub indexed: bool,
}

/// Attribute value that can be string or numeric
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum AttributeValue {
    String(String),
    Number(f64),
    Bool(bool),
    Null,
}

impl AttributeValue {
    /// Convert to EAV storage values (value_str, value_num)
    pub fn to_eav_values(&self) -> (Option<String>, Option<f64>) {
        match self {
            AttributeValue::String(s) => (Some(s.clone()), None),
            AttributeValue::Number(n) => (None, Some(*n)),
            AttributeValue::Bool(b) => (Some(b.to_string()), None),
            AttributeValue::Null => (None, None),
        }
    }

    /// Create from JSON value
    pub fn from_json(value: &serde_json::Value) -> Self {
        match value {
            serde_json::Value::String(s) => AttributeValue::String(s.clone()),
            serde_json::Value::Number(n) => AttributeValue::Number(n.as_f64().unwrap_or(0.0)),
            serde_json::Value::Bool(b) => AttributeValue::Bool(*b),
            serde_json::Value::Null => AttributeValue::Null,
            serde_json::Value::Array(arr) => {
                // Convert array to JSON string
                AttributeValue::String(serde_json::to_string(arr).unwrap_or_default())
            }
            serde_json::Value::Object(obj) => {
                // Convert object to JSON string
                AttributeValue::String(serde_json::to_string(obj).unwrap_or_default())
            }
        }
    }
}

/// Cache for attribute key_name -> key_id mapping
pub struct AttributeKeyCache {
    trace_keys: DashMap<String, i64>,
    span_keys: DashMap<String, i64>,
}

impl AttributeKeyCache {
    /// Create a new empty cache
    pub fn new() -> Self {
        Self { trace_keys: DashMap::new(), span_keys: DashMap::new() }
    }

    /// Load all existing keys from database into cache
    pub async fn load_from_db(&self, pool: &SqlitePool) -> Result<(), OtelError> {
        let rows = sqlx::query_as::<_, (i64, String, String)>(
            "SELECT id, key_name, entity_type FROM attribute_keys",
        )
        .fetch_all(pool)
        .await
        .map_err(|e| OtelError::StorageError(format!("Failed to load attribute keys: {}", e)))?;

        for (id, key_name, entity_type) in rows {
            match entity_type.as_str() {
                "trace" => {
                    self.trace_keys.insert(key_name, id);
                }
                "span" => {
                    self.span_keys.insert(key_name, id);
                }
                _ => {}
            }
        }

        Ok(())
    }

    /// Get key ID from cache, or create if not exists
    pub async fn get_or_create_key(
        &self,
        pool: &SqlitePool,
        key_name: &str,
        key_type: &str,
        entity_type: &str,
    ) -> Result<i64, OtelError> {
        let cache = match entity_type {
            "trace" => &self.trace_keys,
            "span" => &self.span_keys,
            _ => {
                return Err(OtelError::StorageError(format!(
                    "Invalid entity type: {}",
                    entity_type
                )));
            }
        };

        // Check cache first
        if let Some(id) = cache.get(key_name) {
            return Ok(*id);
        }

        // Insert or get from DB
        let id = ensure_key_exists(pool, key_name, key_type, entity_type).await?;
        cache.insert(key_name.to_string(), id);
        Ok(id)
    }

    /// Get key ID from cache, or create if not exists (within a transaction)
    pub async fn get_or_create_key_with_tx(
        &self,
        tx: &mut Transaction<'_, Sqlite>,
        key_name: &str,
        key_type: &str,
        entity_type: &str,
    ) -> Result<i64, OtelError> {
        let cache = match entity_type {
            "trace" => &self.trace_keys,
            "span" => &self.span_keys,
            _ => {
                return Err(OtelError::StorageError(format!(
                    "Invalid entity type: {}",
                    entity_type
                )));
            }
        };

        // Check cache first
        if let Some(id) = cache.get(key_name) {
            return Ok(*id);
        }

        // Insert or get from DB within transaction
        let id = ensure_key_exists_with_tx(tx, key_name, key_type, entity_type).await?;
        cache.insert(key_name.to_string(), id);
        Ok(id)
    }

    /// Get key ID from cache only (no DB lookup)
    pub fn get_key_id(&self, key_name: &str, entity_type: &str) -> Option<i64> {
        match entity_type {
            "trace" => self.trace_keys.get(key_name).map(|v| *v),
            "span" => self.span_keys.get(key_name).map(|v| *v),
            _ => None,
        }
    }

    /// Get all cached keys for an entity type
    pub fn get_all_keys(&self, entity_type: &str) -> Vec<(String, i64)> {
        match entity_type {
            "trace" => self.trace_keys.iter().map(|r| (r.key().clone(), *r.value())).collect(),
            "span" => self.span_keys.iter().map(|r| (r.key().clone(), *r.value())).collect(),
            _ => Vec::new(),
        }
    }
}

impl Default for AttributeKeyCache {
    fn default() -> Self {
        Self::new()
    }
}

/// Ensure an attribute key exists in the database, return its ID
async fn ensure_key_exists(
    pool: &SqlitePool,
    key_name: &str,
    key_type: &str,
    entity_type: &str,
) -> Result<i64, OtelError> {
    let now = chrono::Utc::now().timestamp();

    // Try to insert, or get existing
    sqlx::query(
        "INSERT OR IGNORE INTO attribute_keys (key_name, key_type, entity_type, indexed, created_at)
         VALUES (?, ?, ?, 1, ?)",
    )
    .bind(key_name)
    .bind(key_type)
    .bind(entity_type)
    .bind(now)
    .execute(pool)
    .await
    .map_err(|e| OtelError::StorageError(format!("Failed to insert attribute key: {}", e)))?;

    // Get the ID
    let id: i64 =
        sqlx::query_scalar("SELECT id FROM attribute_keys WHERE key_name = ? AND entity_type = ?")
            .bind(key_name)
            .bind(entity_type)
            .fetch_one(pool)
            .await
            .map_err(|e| {
                OtelError::StorageError(format!("Failed to get attribute key id: {}", e))
            })?;

    Ok(id)
}

/// Ensure an attribute key exists in the database within a transaction, return its ID
async fn ensure_key_exists_with_tx(
    tx: &mut Transaction<'_, Sqlite>,
    key_name: &str,
    key_type: &str,
    entity_type: &str,
) -> Result<i64, OtelError> {
    let now = chrono::Utc::now().timestamp();

    // Try to insert, or get existing
    sqlx::query(
        "INSERT OR IGNORE INTO attribute_keys (key_name, key_type, entity_type, indexed, created_at)
         VALUES (?, ?, ?, 1, ?)",
    )
    .bind(key_name)
    .bind(key_type)
    .bind(entity_type)
    .bind(now)
    .execute(&mut **tx)
    .await
    .map_err(|e| OtelError::StorageError(format!("Failed to insert attribute key: {}", e)))?;

    // Get the ID
    let id: i64 =
        sqlx::query_scalar("SELECT id FROM attribute_keys WHERE key_name = ? AND entity_type = ?")
            .bind(key_name)
            .bind(entity_type)
            .fetch_one(&mut **tx)
            .await
            .map_err(|e| {
                OtelError::StorageError(format!("Failed to get attribute key id: {}", e))
            })?;

    Ok(id)
}

/// Batch insert trace attributes within an existing transaction
pub async fn insert_trace_attributes_batch_with_tx(
    tx: &mut Transaction<'_, Sqlite>,
    attributes: &[(String, i64, Option<String>, Option<f64>)], // (trace_id, key_id, str, num)
) -> Result<(), OtelError> {
    if attributes.is_empty() {
        return Ok(());
    }

    // Insert in chunks for efficiency
    for chunk in attributes.chunks(500) {
        let placeholders: Vec<String> = chunk.iter().map(|_| "(?, ?, ?, ?)".to_string()).collect();
        let sql = format!(
            "INSERT OR REPLACE INTO trace_attributes (trace_id, key_id, value_str, value_num) VALUES {}",
            placeholders.join(", ")
        );

        let mut query = sqlx::query(&sql);
        for (trace_id, key_id, str_val, num_val) in chunk {
            query = query.bind(trace_id).bind(key_id).bind(str_val).bind(num_val);
        }

        query.execute(&mut **tx).await.map_err(|e| {
            OtelError::StorageError(format!("Failed to insert trace attributes: {}", e))
        })?;
    }

    Ok(())
}

/// Batch insert span attributes within an existing transaction
pub async fn insert_span_attributes_batch_with_tx(
    tx: &mut Transaction<'_, Sqlite>,
    attributes: &[(String, i64, Option<String>, Option<f64>)], // (span_id, key_id, str, num)
) -> Result<(), OtelError> {
    if attributes.is_empty() {
        return Ok(());
    }

    for chunk in attributes.chunks(500) {
        let placeholders: Vec<String> = chunk.iter().map(|_| "(?, ?, ?, ?)".to_string()).collect();
        let sql = format!(
            "INSERT OR REPLACE INTO span_attributes (span_id, key_id, value_str, value_num) VALUES {}",
            placeholders.join(", ")
        );

        let mut query = sqlx::query(&sql);
        for (span_id, key_id, str_val, num_val) in chunk {
            query = query.bind(span_id).bind(key_id).bind(str_val).bind(num_val);
        }

        query.execute(&mut **tx).await.map_err(|e| {
            OtelError::StorageError(format!("Failed to insert span attributes: {}", e))
        })?;
    }

    Ok(())
}

/// Get all attributes for a trace
pub async fn get_trace_attributes(
    pool: &SqlitePool,
    trace_id: &str,
) -> Result<std::collections::HashMap<String, serde_json::Value>, OtelError> {
    let rows = sqlx::query_as::<_, (String, Option<String>, Option<f64>)>(
        "SELECT ak.key_name, ta.value_str, ta.value_num
         FROM trace_attributes ta
         JOIN attribute_keys ak ON ta.key_id = ak.id
         WHERE ta.trace_id = ?",
    )
    .bind(trace_id)
    .fetch_all(pool)
    .await
    .map_err(|e| OtelError::StorageError(format!("Failed to get trace attributes: {}", e)))?;

    let mut attrs = std::collections::HashMap::new();
    for (key, str_val, num_val) in rows {
        let value = if let Some(s) = str_val {
            serde_json::Value::String(s)
        } else if let Some(n) = num_val {
            serde_json::json!(n)
        } else {
            serde_json::Value::Null
        };
        attrs.insert(key, value);
    }

    Ok(attrs)
}

/// Get all registered attribute keys
pub async fn get_all_attribute_keys(pool: &SqlitePool) -> Result<Vec<AttributeKey>, OtelError> {
    let rows = sqlx::query_as::<_, (i64, String, String, String, bool)>(
        "SELECT id, key_name, key_type, entity_type, indexed FROM attribute_keys ORDER BY entity_type, key_name",
    )
    .fetch_all(pool)
    .await
    .map_err(|e| OtelError::StorageError(format!("Failed to get attribute keys: {}", e)))?;

    Ok(rows
        .into_iter()
        .map(|(id, key_name, key_type, entity_type, indexed)| AttributeKey {
            id,
            key_name,
            key_type,
            entity_type,
            indexed,
        })
        .collect())
}

/// Get distinct values for an attribute key (for filter dropdowns)
pub async fn get_attribute_distinct_values(
    pool: &SqlitePool,
    key_name: &str,
    entity_type: &str,
    limit: usize,
) -> Result<Vec<String>, OtelError> {
    let table = match entity_type {
        "trace" => "trace_attributes",
        "span" => "span_attributes",
        _ => return Err(OtelError::StorageError(format!("Invalid entity type: {}", entity_type))),
    };

    let sql = format!(
        "SELECT DISTINCT ta.value_str
         FROM {} ta
         JOIN attribute_keys ak ON ta.key_id = ak.id
         WHERE ak.key_name = ? AND ak.entity_type = ? AND ta.value_str IS NOT NULL
         LIMIT ?",
        table
    );

    let rows = sqlx::query_scalar::<_, String>(&sql)
        .bind(key_name)
        .bind(entity_type)
        .bind(limit as i64)
        .fetch_all(pool)
        .await
        .map_err(|e| {
            OtelError::StorageError(format!("Failed to get attribute distinct values: {}", e))
        })?;

    Ok(rows)
}

/// Create a shared attribute key cache
pub fn create_attribute_cache() -> Arc<AttributeKeyCache> {
    Arc::new(AttributeKeyCache::new())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sqlite::schema::SCHEMA;

    async fn setup_test_db() -> SqlitePool {
        let pool = SqlitePool::connect("sqlite::memory:").await.unwrap();
        sqlx::raw_sql(SCHEMA).execute(&pool).await.unwrap();
        pool
    }

    // AttributeValue tests
    #[test]
    fn test_attribute_value_string_to_eav() {
        let value = AttributeValue::String("hello".to_string());
        let (str_val, num_val) = value.to_eav_values();
        assert_eq!(str_val, Some("hello".to_string()));
        assert_eq!(num_val, None);
    }

    #[test]
    fn test_attribute_value_number_to_eav() {
        let value = AttributeValue::Number(42.5);
        let (str_val, num_val) = value.to_eav_values();
        assert_eq!(str_val, None);
        assert_eq!(num_val, Some(42.5));
    }

    #[test]
    fn test_attribute_value_bool_to_eav() {
        let value = AttributeValue::Bool(true);
        let (str_val, num_val) = value.to_eav_values();
        assert_eq!(str_val, Some("true".to_string()));
        assert_eq!(num_val, None);

        let value_false = AttributeValue::Bool(false);
        let (str_val, num_val) = value_false.to_eav_values();
        assert_eq!(str_val, Some("false".to_string()));
        assert_eq!(num_val, None);
    }

    #[test]
    fn test_attribute_value_null_to_eav() {
        let value = AttributeValue::Null;
        let (str_val, num_val) = value.to_eav_values();
        assert_eq!(str_val, None);
        assert_eq!(num_val, None);
    }

    #[test]
    fn test_attribute_value_from_json_string() {
        let json = serde_json::json!("test string");
        let value = AttributeValue::from_json(&json);
        match value {
            AttributeValue::String(s) => assert_eq!(s, "test string"),
            _ => panic!("Expected String variant"),
        }
    }

    #[test]
    fn test_attribute_value_from_json_number_int() {
        let json = serde_json::json!(42);
        let value = AttributeValue::from_json(&json);
        match value {
            AttributeValue::Number(n) => assert_eq!(n, 42.0),
            _ => panic!("Expected Number variant"),
        }
    }

    #[test]
    fn test_attribute_value_from_json_number_float() {
        let json = serde_json::json!(3.5);
        let value = AttributeValue::from_json(&json);
        match value {
            AttributeValue::Number(n) => assert!((n - 3.5).abs() < f64::EPSILON),
            _ => panic!("Expected Number variant"),
        }
    }

    #[test]
    fn test_attribute_value_from_json_bool() {
        let json = serde_json::json!(true);
        let value = AttributeValue::from_json(&json);
        match value {
            AttributeValue::Bool(b) => assert!(b),
            _ => panic!("Expected Bool variant"),
        }
    }

    #[test]
    fn test_attribute_value_from_json_null() {
        let json = serde_json::Value::Null;
        let value = AttributeValue::from_json(&json);
        assert!(matches!(value, AttributeValue::Null));
    }

    #[test]
    fn test_attribute_value_from_json_array() {
        let json = serde_json::json!([1, 2, 3]);
        let value = AttributeValue::from_json(&json);
        match value {
            AttributeValue::String(s) => assert_eq!(s, "[1,2,3]"),
            _ => panic!("Expected String variant for array"),
        }
    }

    #[test]
    fn test_attribute_value_from_json_object() {
        let json = serde_json::json!({"key": "value"});
        let value = AttributeValue::from_json(&json);
        match value {
            AttributeValue::String(s) => assert!(s.contains("key") && s.contains("value")),
            _ => panic!("Expected String variant for object"),
        }
    }

    // AttributeKeyCache tests
    #[test]
    fn test_attribute_key_cache_new() {
        let cache = AttributeKeyCache::new();
        assert!(cache.get_key_id("test", "trace").is_none());
        assert!(cache.get_key_id("test", "span").is_none());
    }

    #[test]
    fn test_attribute_key_cache_default() {
        let cache = AttributeKeyCache::default();
        assert!(cache.get_key_id("test", "trace").is_none());
    }

    #[test]
    fn test_attribute_key_cache_get_all_keys_empty() {
        let cache = AttributeKeyCache::new();
        assert!(cache.get_all_keys("trace").is_empty());
        assert!(cache.get_all_keys("span").is_empty());
        assert!(cache.get_all_keys("invalid").is_empty());
    }

    #[test]
    fn test_attribute_key_cache_get_key_id_invalid_entity() {
        let cache = AttributeKeyCache::new();
        assert!(cache.get_key_id("test", "invalid").is_none());
    }

    #[test]
    fn test_create_attribute_cache() {
        let cache = create_attribute_cache();
        assert!(cache.get_key_id("test", "trace").is_none());
    }

    // Database-backed tests
    #[tokio::test]
    async fn test_attribute_key_cache_load_from_db_empty() {
        let pool = setup_test_db().await;
        let cache = AttributeKeyCache::new();
        cache.load_from_db(&pool).await.unwrap();
        assert!(cache.get_all_keys("trace").is_empty());
        assert!(cache.get_all_keys("span").is_empty());
    }

    #[tokio::test]
    async fn test_attribute_key_cache_get_or_create_key() {
        let pool = setup_test_db().await;
        let cache = AttributeKeyCache::new();

        // Create a new key
        let id1 = cache.get_or_create_key(&pool, "test_key", "string", "trace").await.unwrap();
        assert!(id1 > 0);

        // Get same key should return same ID
        let id2 = cache.get_or_create_key(&pool, "test_key", "string", "trace").await.unwrap();
        assert_eq!(id1, id2);

        // Key should be in cache
        assert_eq!(cache.get_key_id("test_key", "trace"), Some(id1));
    }

    #[tokio::test]
    async fn test_attribute_key_cache_get_or_create_key_different_entities() {
        let pool = setup_test_db().await;
        let cache = AttributeKeyCache::new();

        let trace_id =
            cache.get_or_create_key(&pool, "shared_key", "string", "trace").await.unwrap();
        let span_id = cache.get_or_create_key(&pool, "shared_key", "string", "span").await.unwrap();

        // Different entity types should have different IDs
        assert_ne!(trace_id, span_id);
        assert_eq!(cache.get_key_id("shared_key", "trace"), Some(trace_id));
        assert_eq!(cache.get_key_id("shared_key", "span"), Some(span_id));
    }

    #[tokio::test]
    async fn test_attribute_key_cache_get_or_create_key_invalid_entity() {
        let pool = setup_test_db().await;
        let cache = AttributeKeyCache::new();

        let result = cache.get_or_create_key(&pool, "test", "string", "invalid").await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_attribute_key_cache_load_from_db_with_data() {
        let pool = setup_test_db().await;
        let cache1 = AttributeKeyCache::new();

        // Create some keys
        cache1.get_or_create_key(&pool, "key1", "string", "trace").await.unwrap();
        cache1.get_or_create_key(&pool, "key2", "string", "span").await.unwrap();

        // Create new cache and load from DB
        let cache2 = AttributeKeyCache::new();
        cache2.load_from_db(&pool).await.unwrap();

        // Should have the keys
        assert!(cache2.get_key_id("key1", "trace").is_some());
        assert!(cache2.get_key_id("key2", "span").is_some());
    }

    #[tokio::test]
    async fn test_insert_trace_attributes_batch_empty() {
        let pool = setup_test_db().await;
        let mut tx = pool.begin().await.unwrap();

        let result = insert_trace_attributes_batch_with_tx(&mut tx, &[]).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_insert_trace_attributes_batch() {
        let pool = setup_test_db().await;
        let cache = AttributeKeyCache::new();

        // Create test trace and key
        let now = chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0);
        sqlx::query(
            "INSERT INTO traces (trace_id, service_name, detected_framework, span_count, start_time_ns, has_errors, created_at, updated_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?)"
        )
        .bind("trace-1")
        .bind("test-service")
        .bind("unknown")
        .bind(1)
        .bind(now)
        .bind(false)
        .bind(now)
        .bind(now)
        .execute(&pool)
        .await
        .unwrap();

        let key_id = cache.get_or_create_key(&pool, "test_attr", "string", "trace").await.unwrap();

        let mut tx = pool.begin().await.unwrap();
        let attrs = vec![("trace-1".to_string(), key_id, Some("value1".to_string()), None)];
        insert_trace_attributes_batch_with_tx(&mut tx, &attrs).await.unwrap();
        tx.commit().await.unwrap();

        // Verify attribute was inserted
        let result = get_trace_attributes(&pool, "trace-1").await.unwrap();
        assert_eq!(result.get("test_attr"), Some(&serde_json::json!("value1")));
    }

    #[tokio::test]
    async fn test_insert_span_attributes_batch_empty() {
        let pool = setup_test_db().await;
        let mut tx = pool.begin().await.unwrap();

        let result = insert_span_attributes_batch_with_tx(&mut tx, &[]).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_get_trace_attributes_empty() {
        let pool = setup_test_db().await;
        let result = get_trace_attributes(&pool, "nonexistent").await.unwrap();
        assert!(result.is_empty());
    }

    #[tokio::test]
    async fn test_get_all_attribute_keys_empty() {
        let pool = setup_test_db().await;
        let keys = get_all_attribute_keys(&pool).await.unwrap();
        assert!(keys.is_empty());
    }

    #[tokio::test]
    async fn test_get_all_attribute_keys_with_data() {
        let pool = setup_test_db().await;
        let cache = AttributeKeyCache::new();

        cache.get_or_create_key(&pool, "key1", "string", "trace").await.unwrap();
        cache.get_or_create_key(&pool, "key2", "number", "span").await.unwrap();

        let keys = get_all_attribute_keys(&pool).await.unwrap();
        assert_eq!(keys.len(), 2);
    }

    #[tokio::test]
    async fn test_get_attribute_distinct_values_empty() {
        let pool = setup_test_db().await;
        let values =
            get_attribute_distinct_values(&pool, "nonexistent", "trace", 100).await.unwrap();
        assert!(values.is_empty());
    }

    #[tokio::test]
    async fn test_get_attribute_distinct_values_invalid_entity() {
        let pool = setup_test_db().await;
        let result = get_attribute_distinct_values(&pool, "key", "invalid", 100).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_attribute_key_struct_fields() {
        let key = AttributeKey {
            id: 1,
            key_name: "test_key".to_string(),
            key_type: "string".to_string(),
            entity_type: "trace".to_string(),
            indexed: true,
        };
        assert_eq!(key.id, 1);
        assert_eq!(key.key_name, "test_key");
        assert_eq!(key.key_type, "string");
        assert_eq!(key.entity_type, "trace");
        assert!(key.indexed);
    }

    #[tokio::test]
    async fn test_get_or_create_key_with_tx() {
        let pool = setup_test_db().await;
        let cache = AttributeKeyCache::new();

        let mut tx = pool.begin().await.unwrap();
        let id1 =
            cache.get_or_create_key_with_tx(&mut tx, "tx_key", "string", "trace").await.unwrap();
        assert!(id1 > 0);

        // Same key within same tx
        let id2 =
            cache.get_or_create_key_with_tx(&mut tx, "tx_key", "string", "trace").await.unwrap();
        assert_eq!(id1, id2);

        tx.commit().await.unwrap();

        // Key should persist
        let cache2 = AttributeKeyCache::new();
        cache2.load_from_db(&pool).await.unwrap();
        assert!(cache2.get_key_id("tx_key", "trace").is_some());
    }

    #[tokio::test]
    async fn test_get_or_create_key_with_tx_invalid_entity() {
        let pool = setup_test_db().await;
        let cache = AttributeKeyCache::new();

        let mut tx = pool.begin().await.unwrap();
        let result = cache.get_or_create_key_with_tx(&mut tx, "test", "string", "invalid").await;
        assert!(result.is_err());
    }
}
