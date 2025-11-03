/*!
 * In-Memory Storage Implementation
 *
 * This module provides the core storage layer for Ignix, implementing
 * a concurrent in-memory dictionary using DashMap with a fast hasher.
 */

use crate::protocol::Value;
use dashmap::DashMap;

/// High-performance in-memory dictionary
/// 
/// The core storage structure that holds all key-value pairs in memory.
/// Uses SwissTable (hashbrown) with AHash for fast lookups and supports all Redis-compatible operations.
#[derive(Default)]
pub struct Dict {
    /// Concurrent DashMap for optimal performance (sharded locking)
    pub(crate) inner: DashMap<Vec<u8>, Value>,
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
    /// * `k` - Key as owned byte vector
    /// * `v` - Value to store
    #[inline]
    pub fn set(&self, k: Vec<u8>, v: Value) {
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
    /// * `from` - Current key name as owned byte vector
    /// * `to` - New key name as owned byte vector
    /// 
    /// # Returns
    /// * `true` if rename was successful
    /// * `false` if source key didn't exist
    #[inline]
    pub fn rename(&self, from: Vec<u8>, to: Vec<u8>) -> bool {
        // Handle edge case where source and destination are the same
        if from == to {
            return true;
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

    /// Atomically increment an integer-like value stored under key, creating it if missing
    pub fn incr(&self, k: &[u8]) -> i64 {
        use dashmap::mapref::entry::Entry;
        match self.inner.entry(k.to_vec()) {
            Entry::Occupied(mut e) => match e.get_mut() {
                Value::Int(i) => {
                    *i += 1;
                    *i
                }
                Value::Str(s) => {
                    let mut n = std::str::from_utf8(s)
                        .ok()
                        .and_then(|x| x.parse::<i64>().ok())
                        .unwrap_or(0);
                    n += 1;
                    *s = n.to_string().into_bytes();
                    n
                }
                _ => 0,
            },
            Entry::Vacant(v) => {
                v.insert(Value::Int(1));
                1
            }
        }
    }
}