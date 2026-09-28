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

    /// Lock the shards of `keys` with `lock`, in ascending order, and
    /// return the guards and each key's hash with the index of its shard's
    /// guard.
    fn lock_shards<'k, G>(
        &self,
        keys: impl IntoIterator<Item = &'k [u8]>,
        lock: impl Fn(usize) -> G,
    ) -> (Vec<G>, Vec<(u64, usize)>) {
        // The shards in use, as a bitmap, and how many there are
        let mut used = [0u64; SHARDS / 64];
        let mut count = 0;
        let mut keys: Vec<(u64, usize)> = keys
            .into_iter()
            .map(|k| {
                let (hash, shard) = self.locate(k);
                let (word, bit) = (shard / 64, 1 << (shard % 64));
                count += usize::from(used[word] & bit == 0);
                used[word] |= bit;
                (hash, shard)
            })
            .collect();
        let mut guard_of = [0u16; SHARDS];
        let mut guards = Vec::with_capacity(count);
        for (word, &bits) in used.iter().enumerate() {
            let mut bits = bits;
            while bits != 0 {
                let shard = word * 64 + bits.trailing_zeros() as usize;
                bits &= bits - 1;
                guard_of[shard] = guards.len() as u16;
                guards.push(lock(shard));
            }
        }
        for (_, slot) in &mut keys {
            *slot = usize::from(guard_of[*slot]);
        }
        (guards, keys)
    }

    /// Write-lock the shards of `keys` together, for a change to several
    /// keys that other threads must not see half done. The dictionary must
    /// not be used while the locks are held.
    ///
    /// Several shards are only ever locked in ascending order, here and in
    /// [`Dict::read_many`], so threads locking overlapping keys cannot
    /// deadlock; every other operation holds one lock at a time.
    pub(crate) fn lock_keys<'k>(&self, keys: impl IntoIterator<Item = &'k [u8]>) -> LockedKeys<'_> {
        let (guards, keys) = self.lock_shards(keys, |shard| self.write_shard(shard));
        LockedKeys {
            hasher: &self.hasher,
            guards,
            keys,
        }
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

    /// Call `f` with the value stored under `k` while its shard's read lock
    /// is held, e.g. to copy it into a reply without cloning it. `f` must not
    /// use the dictionary.
    #[inline]
    pub(crate) fn read<R>(&self, k: &[u8], f: impl FnOnce(Option<&Value>) -> R) -> R {
        let (hash, shard) = self.locate(k);
        let table = self.read_shard(shard);
        f(table
            .raw_entry()
            .from_key_hashed_nocheck(hash, k)
            .map(|(_, entry)| &entry.value))
    }

    /// Call `f` with the value of each of `keys`, in order, while the shards
    /// of all of them are read-locked, so no other thread changes any of the
    /// keys in between (MGET, EXISTS). `f` must not use the dictionary.
    pub(crate) fn read_many(&self, keys: &[Bytes], mut f: impl FnMut(Option<&Value>)) {
        if let [key] = keys {
            return self.read(key, f);
        }
        let keys_bytes = keys.iter().map(|k| &k[..]);
        let (guards, located) = self.lock_shards(keys_bytes, |shard| self.read_shard(shard));
        for (key, &(hash, guard)) in keys.iter().zip(&located) {
            f(guards[guard]
                .raw_entry()
                .from_key_hashed_nocheck(hash, &key[..])
                .map(|(_, entry)| &entry.value));
        }
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

    /// Set several keys at once (MSET), storing `encode(value)` for each:
    /// other threads see either none or all of the new values. A repeated key
    /// keeps its last value.
    pub(crate) fn set_many<V>(&self, pairs: Vec<(Bytes, V)>, encode: impl Fn(V) -> Value) {
        let mut locked = self.lock_keys(pairs.iter().map(|(k, _)| &k[..]));
        for (i, (k, v)) in pairs.into_iter().enumerate() {
            locked.insert(i, k, Entry::new(encode(v)));
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

    /// Delete several keys at once (DEL): other threads see either none or
    /// all of them removed. Only the keys that were removed stay in `keys`,
    /// a repeated key once.
    pub(crate) fn del_many(&self, keys: &mut Vec<Bytes>) {
        if let [key] = &keys[..] {
            if !self.del(key) {
                keys.clear();
            }
            return;
        }
        let mut locked = self.lock_keys(keys.iter().map(|k| &k[..]));
        let mut i = 0;
        // `retain` visits the keys once each, in order
        keys.retain(|key| {
            let removed = locked.remove(i, key).is_some();
            i += 1;
            removed
        });
    }

    /// Rename a key
    ///
    /// Moves the value from the old key to the new key, replacing any value
    /// the new key had. Other threads see the value under exactly one of the
    /// two names.
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

        let mut locked = self.lock_keys([&from[..], &to[..]]);
        let Some(entry) = locked.remove(0, &from) else {
            return false;
        };
        locked.insert(1, to, entry);
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

/// Write locks on the shards of a list of keys, from [`Dict::lock_keys`]
///
/// A key is addressed by its position in that list, and every method must be
/// given the key at that position.
pub(crate) struct LockedKeys<'a> {
    hasher: &'a RandomState,
    /// The locked shards, in ascending order
    guards: Vec<RwLockWriteGuard<'a, Table>>,
    /// Each key's hash and the index of its shard's guard
    keys: Vec<(u64, usize)>,
}

impl LockedKeys<'_> {
    /// The hash and the table of key `i`
    fn table(&mut self, i: usize, key: &[u8]) -> (u64, &mut Table) {
        let (hash, guard) = self.keys[i];
        debug_assert_eq!(hash, self.hasher.hash_one(key), "key {i} was not locked");
        (hash, &mut self.guards[guard])
    }

    /// Remove key `i`, returning its entry
    pub(crate) fn remove(&mut self, i: usize, key: &[u8]) -> Option<Entry> {
        let (hash, table) = self.table(i, key);
        match table.raw_entry_mut().from_key_hashed_nocheck(hash, key) {
            RawEntryMut::Occupied(entry) => Some(entry.remove()),
            RawEntryMut::Vacant(_) => None,
        }
    }

    /// Store `entry` under key `i`, replacing any previous entry
    pub(crate) fn insert(&mut self, i: usize, key: Bytes, entry: Entry) {
        let (hash, table) = self.table(i, &key);
        match table
            .raw_entry_mut()
            .from_key_hashed_nocheck(hash, &key[..])
        {
            RawEntryMut::Occupied(mut slot) => *slot.get_mut() = entry,
            RawEntryMut::Vacant(slot) => {
                slot.insert_hashed_nocheck(hash, key, entry);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, Ordering};

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
    fn set_many_and_del_many_with_keys_in_most_shards() {
        let dict = Dict::default();
        let keys: Vec<Bytes> = (0..5_000).map(|i| key(&format!("key:{i}"))).collect();
        let shards: std::collections::HashSet<usize> =
            keys.iter().map(|k| dict.locate(k).1).collect();
        assert!(shards.contains(&0) && shards.contains(&(SHARDS - 1)));
        let pairs = keys.iter().zip(0..).map(|(k, i)| (k.clone(), i)).collect();
        dict.set_many(pairs, Value::Int);
        let mut values = Vec::new();
        dict.read_many(&keys, |value| values.push(value.cloned()));
        let expected: Vec<_> = (0..5_000).map(|i| Some(Value::Int(i))).collect();
        assert_eq!(values, expected);
        let mut removed = keys.clone();
        dict.del_many(&mut removed);
        assert_eq!(removed, keys);
        assert!(dict.is_empty());
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

    /// Run `write` while another thread keeps reading `keys` with
    /// `read_many` and calls `check` with their values.
    fn check_while_writing(
        dict: &Dict,
        keys: &[Bytes],
        check: impl Fn(&[Option<Value>]) + Sync,
        write: impl FnOnce(),
    ) {
        let done = AtomicBool::new(false);
        std::thread::scope(|s| {
            let reader = s.spawn(|| {
                let mut values = Vec::new();
                while !done.load(Ordering::Relaxed) {
                    values.clear();
                    dict.read_many(keys, |value| values.push(value.cloned()));
                    check(&values);
                }
            });
            write();
            done.store(true, Ordering::Relaxed);
            reader.join().unwrap();
        });
    }

    #[test]
    fn opposite_renames_do_not_deadlock_or_lose_the_key() {
        let dict = Dict::default();
        let (a, same_shard, other_shard) = keys_by_shard(&dict);
        for b in [same_shard, other_shard] {
            dict.clear();
            dict.set(a.clone(), Value::Int(1));
            let exactly_one = |values: &[Option<Value>]| {
                let found = values.iter().filter(|v| v.is_some()).count();
                assert_eq!(found, 1, "{values:?}");
            };
            check_while_writing(&dict, &[a.clone(), b.clone()], exactly_one, || {
                std::thread::scope(|s| {
                    for (from, to) in [(&a, &b), (&b, &a)] {
                        let dict = &dict;
                        s.spawn(move || {
                            for _ in 0..20_000 {
                                dict.rename(from.clone(), to.clone());
                            }
                        });
                    }
                });
            });
            assert_eq!(dict.len(), 1);
            assert!(dict.exists(&a) != dict.exists(&b));
        }
    }

    #[test]
    fn mset_and_del_are_seen_all_at_once() {
        let dict = Dict::default();
        // Two keys in one shard and one in another
        let (a, b, c) = keys_by_shard(&dict);
        let keys = [a, b, c];
        let all_same = |values: &[Option<Value>]| {
            assert!(values.windows(2).all(|w| w[0] == w[1]), "{values:?}");
        };
        check_while_writing(&dict, &keys, all_same, || {
            for i in 0..20_000 {
                let pairs = keys.iter().map(|k| (k.clone(), i)).collect();
                dict.set_many(pairs, Value::Int);
                if i % 2 == 1 {
                    let mut removed = keys.to_vec();
                    dict.del_many(&mut removed);
                    assert_eq!(removed.len(), 3);
                }
            }
        });
    }

    #[test]
    fn repeated_keys_in_set_many_and_del_many() {
        let dict = Dict::default();
        let (a, b, c) = keys_by_shard(&dict);
        dict.set_many(
            vec![
                (a.clone(), Value::Int(1)),
                (c.clone(), Value::Int(2)),
                (a.clone(), Value::Int(3)),
            ],
            |v| v,
        );
        assert_eq!(dict.get(&a), Some(Value::Int(3)));
        assert_eq!(dict.len(), 2);

        let mut keys = vec![a.clone(), b.clone(), a.clone(), c.clone()];
        dict.del_many(&mut keys);
        assert_eq!(keys, [&a, &c]);
        assert!(dict.is_empty());

        // A single key does not go through `lock_keys`
        dict.set(a.clone(), Value::Int(1));
        let mut keys = vec![a.clone()];
        dict.del_many(&mut keys);
        assert_eq!(keys, [&a]);
        dict.del_many(&mut keys);
        assert!(keys.is_empty());
    }
}
