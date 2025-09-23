/*!
 * In-Memory Storage Implementation
 * 
 * This module provides the core storage layer for Ignix, implementing
 * a high-performance in-memory dictionary using AHash for fast lookups.
 */

use crate::protocol::Value;
use std::collections::HashMap;
use std::hash::BuildHasherDefault;

// Use AHash for better performance than default hasher
// AHash is specifically designed for hash tables and provides
// better performance and security than the default SipHash
type AHash = BuildHasherDefault<ahash::AHasher>;

/// High-performance in-memory dictionary
/// 
/// The core storage structure that holds all key-value pairs in memory.
/// Uses AHash for fast lookups and supports all Redis-compatible operations.
#[derive(Default)]
pub struct Dict {
    /// Internal HashMap with AHash for optimal performance
    pub(crate) inner: HashMap<Vec<u8>, Value, AHash>,
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
    pub fn get(&self, k: &[u8]) -> Option<&Value> {
        self.inner.get(k)
    }
    
    /// Get a mutable value by key
    /// 
    /// Used for operations that need to modify values in-place,
    /// such as INCR operations.
    /// 
    /// # Arguments
    /// * `k` - Key to lookup as byte slice
    /// 
    /// # Returns
    /// * `Some(&mut Value)` if key exists
    /// * `None` if key doesn't exist
    #[inline]
    pub fn get_mut(&mut self, k: &[u8]) -> Option<&mut Value> {
        self.inner.get_mut(k)
    }
    
    /// Set a key-value pair
    /// 
    /// Inserts or updates a key with the given value.
    /// If key already exists, the old value is replaced.
    /// 
    /// # Arguments
    /// * `k` - Key as owned byte vector
    /// * `v` - Value to store
    #[inline]
    pub fn set(&mut self, k: Vec<u8>, v: Value) {
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
    pub fn del(&mut self, k: &[u8]) -> bool {
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
    pub fn rename(&mut self, from: Vec<u8>, to: Vec<u8>) -> bool {
        // Handle edge case where source and destination are the same
        if from == to {
            return true;
        }
        
        // Try to remove the source key and get its value
        if let Some(v) = self.inner.remove(&from) {
            // Insert the value with the new key
            self.inner.insert(to, v);
            true
        } else {
            // Source key didn't exist
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
}