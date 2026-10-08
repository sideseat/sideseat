//! A hash of a JSON value that is the same in every process, for ids built from content.

use serde_json::Value as JsonValue;

/// FNV-1a hash constants (64-bit).
///
/// FNV-1a is a simple, non-cryptographic hash that's deterministic across
/// processes and platforms. Used for generating synthetic IDs.
///
/// 64 bits rather than 32, because the id is an identity: correlation marks every call carrying a matching id
/// answered, so two different calls of one tool that collide share one answer. At 32 bits a collision is a
/// matter of tens of thousands of calls, and one is easy to construct (`{"x":"f7e1b3d7"}` and
/// `{"x":"df245afa"}` both hashed to `7df5c5c8`).
const FNV_OFFSET_BASIS: u64 = 0xcbf2_9ce4_8422_2325;
const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;

/// Compute a stable FNV-1a hash of a byte slice.
///
/// This hash is deterministic across process restarts and platforms,
/// unlike `DefaultHasher` which uses random seeding.
fn fnv1a_hash(data: &[u8]) -> u64 {
    let mut hash = FNV_OFFSET_BASIS;
    for byte in data {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(FNV_PRIME);
    }
    hash
}

/// Compute a short hash string (16 hex chars) for a JSON value.
///
/// Used for generating synthetic IDs for providers that don't supply them (e.g., Gemini).
/// This hash is **deterministic across process restarts and platforms**, making it
/// suitable for correlating tool calls and results across server restarts.
pub(crate) fn compute_short_hash(value: &JsonValue) -> String {
    // Serialize to JSON string (deterministic ordering from serde_json)
    let json_str = serde_json::to_string(value).unwrap_or_default();
    let hash = fnv1a_hash(json_str.as_bytes());
    format!("{hash:016x}")
}
