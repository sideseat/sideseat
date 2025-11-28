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

/// Batch insert trace attributes
pub async fn insert_trace_attributes_batch(
    pool: &SqlitePool,
    attributes: &[(String, i64, Option<String>, Option<f64>)], // (trace_id, key_id, str, num)
) -> Result<(), OtelError> {
    if attributes.is_empty() {
        return Ok(());
    }

    let mut tx = pool
        .begin()
        .await
        .map_err(|e| OtelError::StorageError(format!("Failed to begin transaction: {}", e)))?;

    insert_trace_attributes_batch_with_tx(&mut tx, attributes).await?;

    tx.commit()
        .await
        .map_err(|e| OtelError::StorageError(format!("Failed to commit transaction: {}", e)))?;

    Ok(())
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

/// Batch insert span attributes
pub async fn insert_span_attributes_batch(
    pool: &SqlitePool,
    attributes: &[(String, i64, Option<String>, Option<f64>)], // (span_id, key_id, str, num)
) -> Result<(), OtelError> {
    if attributes.is_empty() {
        return Ok(());
    }

    let mut tx = pool
        .begin()
        .await
        .map_err(|e| OtelError::StorageError(format!("Failed to begin transaction: {}", e)))?;

    insert_span_attributes_batch_with_tx(&mut tx, attributes).await?;

    tx.commit()
        .await
        .map_err(|e| OtelError::StorageError(format!("Failed to commit transaction: {}", e)))?;

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

/// Get all attributes for a span
pub async fn get_span_attributes(
    pool: &SqlitePool,
    span_id: &str,
) -> Result<std::collections::HashMap<String, serde_json::Value>, OtelError> {
    let rows = sqlx::query_as::<_, (String, Option<String>, Option<f64>)>(
        "SELECT ak.key_name, sa.value_str, sa.value_num
         FROM span_attributes sa
         JOIN attribute_keys ak ON sa.key_id = ak.id
         WHERE sa.span_id = ?",
    )
    .bind(span_id)
    .fetch_all(pool)
    .await
    .map_err(|e| OtelError::StorageError(format!("Failed to get span attributes: {}", e)))?;

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
