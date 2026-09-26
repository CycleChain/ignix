/*!
 * In-Memory Storage Implementation
 *
 * This module provides the core storage layer for Ignix, implementing
 * a concurrent in-memory dictionary using DashMap with a fast hasher.
 */

use crate::protocol::{parse_canonical_i64, Value};
use bytes::Bytes;
use dashmap::DashMap;
use std::fmt;

/// Error returned by [`Dict::incr`]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum IncrError {
    /// The stored value is not a canonical 64-bit integer string
    NotAnInteger,
    /// The result would not fit in an `i64`
    Overflow,
}

impl IncrError {
    /// The Redis error line for this error
    pub fn as_str(&self) -> &'static str {
        match self {
            IncrError::NotAnInteger => "ERR value is not an integer or out of range",
            IncrError::Overflow => "ERR increment or decrement would overflow",
        }
    }
}

impl fmt::Display for IncrError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl std::error::Error for IncrError {}

/// High-performance in-memory dictionary
///
/// The core storage structure that holds all key-value pairs in memory.
/// Uses DashMap (sharded hash tables) with its default hasher and supports all Redis-compatible operations.
#[derive(Default)]
pub struct Dict {
    /// Concurrent DashMap for optimal performance (sharded locking)
    pub(crate) inner: DashMap<Bytes, Value>,
}

impl Dict {
    /// Get a value by key (immutable reference)
    ///
    /// # Arguments
    /// * `k` - Key to lookup as byte slice
    ///
    /// # Returns
    /// * `Some(&Value)` if key exists
    /// * `None` if key doesn't exist
    #[inline]
    pub fn get(&self, k: &[u8]) -> Option<Value> {
        self.inner.get(k).map(|v| v.clone())
    }

    // note: Direct mutable references are not exposed; use entry APIs for atomic updates.

    /// Set a key-value pair
    ///
    /// Inserts or updates a key with the given value.
    /// If key already exists, the old value is replaced.
    ///
    /// # Arguments
    /// * `k` - Key as owned Bytes
    /// * `v` - Value to store
    #[inline]
    pub fn set(&self, k: Bytes, v: Value) {
        self.inner.insert(k, v);
    }

    /// Delete a key
    ///
    /// Removes the key and its associated value from the dictionary.
    ///
    /// # Arguments
    /// * `k` - Key to delete as byte slice
    ///
    /// # Returns
    /// * `true` if key existed and was deleted
    /// * `false` if key didn't exist
    #[inline]
    pub fn del(&self, k: &[u8]) -> bool {
        self.inner.remove(k).is_some()
    }

    /// Rename a key
    ///
    /// Moves the value from the old key to the new key.
    /// The old key is deleted and the new key gets the value.
    ///
    /// # Arguments
    /// * `from` - Current key name as owned Bytes
    /// * `to` - New key name as owned Bytes
    ///
    /// # Returns
    /// * `true` if rename was successful
    /// * `false` if source key didn't exist
    #[inline]
    pub fn rename(&self, from: Bytes, to: Bytes) -> bool {
        // Renaming a key onto itself only succeeds if the key exists (Redis)
        if from == to {
            return self.inner.contains_key(&from);
        }

        // Simple remove-then-insert; note this is not atomic across shards
        if let Some((_, v)) = self.inner.remove(&from) {
            self.inner.insert(to, v);
            true
        } else {
            false
        }
    }

    /// Check if a key exists
    ///
    /// Tests for key existence without retrieving the value.
    ///
    /// # Arguments
    /// * `k` - Key to check as byte slice
    ///
    /// # Returns
    /// * `true` if key exists
    /// * `false` if key doesn't exist
    #[inline]
    pub fn exists(&self, k: &[u8]) -> bool {
        self.inner.contains_key(k)
    }

    /// Atomically increment the integer stored under `key`, creating it with
    /// value 1 if it is missing.
    ///
    /// Like Redis, fails without changing the stored value when it is not a
    /// canonical integer string or when the result would overflow an `i64`.
    pub fn incr(&self, key: Bytes) -> Result<i64, IncrError> {
        self.incr_by(key, 1)
    }

    /// Atomically add `delta` to the integer stored under `key`, starting from
    /// 0 if it is missing (INCRBY, DECRBY and DECR).
    ///
    /// Fails without changing the stored value like [`Dict::incr`].
    pub fn incr_by(&self, key: Bytes, delta: i64) -> Result<i64, IncrError> {
        use dashmap::mapref::entry::Entry;
        match self.inner.entry(key) {
            Entry::Occupied(mut e) => {
                let current = match e.get() {
                    Value::Int(i) => *i,
                    Value::Str(s) | Value::Blob(s) => {
                        parse_canonical_i64(s).ok_or(IncrError::NotAnInteger)?
                    }
                };
                let next = current.checked_add(delta).ok_or(IncrError::Overflow)?;
                *e.get_mut() = Value::Int(next);
                Ok(next)
            }
            Entry::Vacant(v) => {
                v.insert(Value::Int(delta));
                Ok(delta)
            }
        }
    }
}
