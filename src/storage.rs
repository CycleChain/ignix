/*!
 * In-Memory Storage Implementation
 *
 * The keyspace is split into `SHARDS` hash tables, each behind its own
 * read-write lock. A key's hash picks its shard and is reused inside the
 * shard's table, so every operation hashes the key once.
 */

use crate::protocol::{parse_canonical_i64, Value};
use bytes::Bytes;
use crossbeam::utils::CachePadded;
use hashbrown::hash_map::RawEntryMut;
use hashbrown::HashMap;
use std::collections::hash_map::RandomState;
use std::fmt;
use std::hash::BuildHasher;
use std::sync::{PoisonError, RwLock, RwLockReadGuard, RwLockWriteGuard};

/// Number of shards of the keyspace (a power of two)
const SHARDS: usize = 1024;

/// The shard index comes from bits 47..57 of the key's hash: below the top 7
/// bits hashbrown uses as tags and above the low bits it uses for buckets.
const SHARD_SHIFT: u32 = 64 - 7 - SHARDS.trailing_zeros();

/// A stored value
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Entry {
    pub(crate) value: Value,
    /// Absolute expiry time in unix milliseconds; 0 means no expiry
    pub(crate) expires_at: u64,
}

impl Entry {
    fn new(value: Value) -> Self {
        Self {
            value,
            expires_at: 0,
        }
    }
}

/// One shard: keys hashed with the `Dict`'s hasher
type Table = HashMap<Bytes, Entry, RandomState>;

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

/// Concurrent in-memory dictionary
///
/// The core storage structure that holds all key-value pairs in memory, in
/// 1024 hash tables (shards) with one read-write lock each. Keys are hashed
/// with SipHash and random keys (std `RandomState`), like std's `HashMap`.
pub struct Dict {
    hasher: RandomState,
    shards: Box<[CachePadded<RwLock<Table>>]>,
}

impl Default for Dict {
    fn default() -> Self {
        let hasher = RandomState::new();
        let shards = (0..SHARDS)
            .map(|_| CachePadded::new(RwLock::new(Table::with_hasher(hasher.clone()))))
            .collect();
        Self { hasher, shards }
    }
}

impl fmt::Debug for Dict {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Dict").field("keys", &self.len()).finish()
    }
}

impl Dict {
    /// The key's hash and the index of its shard
    #[inline]
    fn locate(&self, key: &[u8]) -> (u64, usize) {
        let hash = self.hasher.hash_one(key);
        (hash, ((hash >> SHARD_SHIFT) as usize) & (SHARDS - 1))
    }

    #[inline]
    fn read_shard(&self, index: usize) -> RwLockReadGuard<'_, Table> {
        // A panic while a lock was held cannot leave a table half-updated in
        // a way that matters here, so a poisoned lock is simply used.
        self.shards[index]
            .read()
            .unwrap_or_else(PoisonError::into_inner)
    }

    #[inline]
    fn write_shard(&self, index: usize) -> RwLockWriteGuard<'_, Table> {
        self.shards[index]
            .write()
            .unwrap_or_else(PoisonError::into_inner)
    }

    /// Number of keys
    pub fn len(&self) -> usize {
        (0..SHARDS).map(|i| self.read_shard(i).len()).sum()
    }

    /// Whether the dictionary holds no keys
    pub fn is_empty(&self) -> bool {
        (0..SHARDS).all(|i| self.read_shard(i).is_empty())
    }

    /// Remove every key
    pub fn clear(&self) {
        for i in 0..SHARDS {
            self.write_shard(i).clear();
        }
    }

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
        let (hash, shard) = self.locate(k);
        self.read_shard(shard)
            .raw_entry()
            .from_key_hashed_nocheck(hash, k)
            .map(|(_, entry)| entry.value.clone())
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
        let (hash, shard) = self.locate(&k);
        let mut table = self.write_shard(shard);
        match table.raw_entry_mut().from_key_hashed_nocheck(hash, &k[..]) {
            RawEntryMut::Occupied(mut entry) => *entry.get_mut() = Entry::new(v),
            RawEntryMut::Vacant(entry) => {
                entry.insert_hashed_nocheck(hash, k, Entry::new(v));
            }
        }
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
        let (hash, shard) = self.locate(k);
        match self
            .write_shard(shard)
            .raw_entry_mut()
            .from_key_hashed_nocheck(hash, k)
        {
            RawEntryMut::Occupied(entry) => {
                entry.remove();
                true
            }
            RawEntryMut::Vacant(_) => false,
        }
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
            return self.exists(&from);
        }

        // Remove, then insert; readers can see neither name in between
        let (hash, shard) = self.locate(&from);
        let removed = match self
            .write_shard(shard)
            .raw_entry_mut()
            .from_key_hashed_nocheck(hash, &from[..])
        {
            RawEntryMut::Occupied(entry) => Some(entry.remove()),
            RawEntryMut::Vacant(_) => None,
        };
        let Some(entry) = removed else {
            return false;
        };
        let (hash, shard) = self.locate(&to);
        match self
            .write_shard(shard)
            .raw_entry_mut()
            .from_key_hashed_nocheck(hash, &to[..])
        {
            RawEntryMut::Occupied(mut slot) => *slot.get_mut() = entry,
            RawEntryMut::Vacant(slot) => {
                slot.insert_hashed_nocheck(hash, to, entry);
            }
        }
        true
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
        let (hash, shard) = self.locate(k);
        self.read_shard(shard)
            .raw_entry()
            .from_key_hashed_nocheck(hash, k)
            .is_some()
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
        let (hash, shard) = self.locate(&key);
        let mut table = self.write_shard(shard);
        match table
            .raw_entry_mut()
            .from_key_hashed_nocheck(hash, &key[..])
        {
            RawEntryMut::Occupied(mut slot) => {
                let entry = slot.get_mut();
                let current = match &entry.value {
                    Value::Int(i) => *i,
                    Value::Str(s) | Value::Blob(s) => {
                        parse_canonical_i64(s).ok_or(IncrError::NotAnInteger)?
                    }
                };
                let next = current.checked_add(delta).ok_or(IncrError::Overflow)?;
                entry.value = Value::Int(next);
                Ok(next)
            }
            RawEntryMut::Vacant(slot) => {
                slot.insert_hashed_nocheck(hash, key, Entry::new(Value::Int(delta)));
                Ok(delta)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(s: &str) -> Bytes {
        Bytes::copy_from_slice(s.as_bytes())
    }

    /// Two keys in the same shard, and one in another shard
    fn keys_by_shard(dict: &Dict) -> (Bytes, Bytes, Bytes) {
        let first = key("k0");
        let shard = dict.locate(&first).1;
        let mut same = None;
        let mut other = None;
        for i in 1.. {
            let k = key(&format!("k{i}"));
            if dict.locate(&k).1 == shard {
                same.get_or_insert(k);
            } else {
                other.get_or_insert(k);
            }
            if same.is_some() && other.is_some() {
                break;
            }
        }
        (first, same.unwrap(), other.unwrap())
    }

    #[test]
    fn entry_stays_small() {
        assert_eq!(std::mem::size_of::<Entry>(), 48);
    }

    #[test]
    fn keys_survive_table_growth() {
        let dict = Dict::default();
        for i in 0..200_000 {
            dict.set(key(&format!("key:{i}")), Value::Int(i));
        }
        assert_eq!(dict.len(), 200_000);
        for i in 0..200_000 {
            assert_eq!(dict.get(format!("key:{i}").as_bytes()), Some(Value::Int(i)));
        }
        dict.clear();
        assert!(dict.is_empty());
    }

    #[test]
    fn rename_within_and_across_shards() {
        let dict = Dict::default();
        let (a, same_shard, other_shard) = keys_by_shard(&dict);
        dict.set(a.clone(), Value::Str(key("v")));
        assert!(dict.rename(a.clone(), same_shard.clone()));
        assert!(dict.rename(same_shard.clone(), other_shard.clone()));
        assert_eq!(dict.get(&other_shard), Some(Value::Str(key("v"))));
        assert!(!dict.exists(&a) && !dict.exists(&same_shard));
        assert_eq!(dict.len(), 1);
    }
}
