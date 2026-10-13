// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! The session cache behind OSC 99's `g=` key.
//!
//! A program may transmit icon bytes once under a cache key (`g=`) and refer
//! to that key in later notifications with the bytes omitted. Escape-sequence
//! input is untrusted, so the cache is bounded: at most
//! [`MAX_ICON_CACHE_ENTRIES`] entries and [`MAX_ICON_CACHE_BYTES`] bytes in
//! total. When a bound would be exceeded the **least recently used** entry is
//! evicted first. Both inserting a key and looking it up make it the most
//! recently used.

use std::collections::VecDeque;

/// Max number of distinct `g=` icon entries retained per session.
const MAX_ICON_CACHE_ENTRIES: usize = 64;
/// Max total icon bytes retained per session (4 MiB).
const MAX_ICON_CACHE_BYTES: usize = 4 * 1024 * 1024;

/// A bounded, least-recently-used cache of icon bytes keyed by `g=`.
///
/// With at most [`MAX_ICON_CACHE_ENTRIES`] entries a linear scan is cheaper
/// than a second index, so the entries live in one deque ordered from least to
/// most recently used.
#[derive(Debug, Default)]
pub struct Osc99IconCache {
    /// Least recently used at the front, most recently used at the back.
    entries: VecDeque<(String, Vec<u8>)>,
    /// The sum of the entries' byte lengths.
    total_bytes: usize,
}

impl Osc99IconCache {
    /// Cache `bytes` under `key`, making it the most recently used entry.
    ///
    /// An existing entry under `key` is replaced. Least recently used entries
    /// are evicted until the new entry fits both bounds. A single icon larger
    /// than the whole byte budget is not cacheable: it is dropped, and so is
    /// any entry already under `key` (a later `g=`-only reference then misses
    /// and the program must re-transmit, rather than getting stale bytes).
    pub fn insert(&mut self, key: String, bytes: Vec<u8>) {
        self.remove(&key);
        if bytes.len() > MAX_ICON_CACHE_BYTES {
            return;
        }
        while !self.entries.is_empty()
            && (self.entries.len() >= MAX_ICON_CACHE_ENTRIES
                || self.total_bytes.saturating_add(bytes.len()) > MAX_ICON_CACHE_BYTES)
        {
            if let Some((_, evicted)) = self.entries.pop_front() {
                self.total_bytes = self.total_bytes.saturating_sub(evicted.len());
            }
        }
        self.total_bytes = self.total_bytes.saturating_add(bytes.len());
        self.entries.push_back((key, bytes));
    }

    /// The bytes cached under `key`, making that entry the most recently used.
    pub fn get(&mut self, key: &str) -> Option<&[u8]> {
        let position = self.entries.iter().position(|(k, _)| k == key)?;
        let entry = self.entries.remove(position)?;
        self.entries.push_back(entry);
        self.entries.back().map(|(_, bytes)| bytes.as_slice())
    }

    /// Drop the entry under `key`, if any.
    fn remove(&mut self, key: &str) {
        if let Some(position) = self.entries.iter().position(|(k, _)| k == key)
            && let Some((_, removed)) = self.entries.remove(position)
        {
            self.total_bytes = self.total_bytes.saturating_sub(removed.len());
        }
    }

    /// The number of cached entries.
    #[cfg(test)]
    fn len(&self) -> usize {
        self.entries.len()
    }

    /// The keys from least to most recently used.
    #[cfg(test)]
    fn keys_oldest_first(&self) -> Vec<&str> {
        self.entries.iter().map(|(key, _)| key.as_str()).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(i: usize) -> String {
        format!("k{i}")
    }

    #[test]
    fn a_stored_entry_is_returned() {
        let mut cache = Osc99IconCache::default();
        cache.insert("a".to_owned(), vec![1, 2, 3]);

        assert_eq!(cache.get("a"), Some([1u8, 2, 3].as_slice()));
        assert_eq!(cache.get("missing"), None);
    }

    #[test]
    fn inserting_an_existing_key_replaces_it_and_makes_it_most_recent() {
        let mut cache = Osc99IconCache::default();
        cache.insert("a".to_owned(), vec![1]);
        cache.insert("b".to_owned(), vec![2]);
        cache.insert("a".to_owned(), vec![3, 3]);

        assert_eq!(cache.keys_oldest_first(), ["b", "a"]);
        assert_eq!(cache.get("a"), Some([3u8, 3].as_slice()));
        assert_eq!(cache.total_bytes, 3);
    }

    #[test]
    fn a_hit_makes_the_entry_most_recent() {
        let mut cache = Osc99IconCache::default();
        for name in ["a", "b", "c"] {
            cache.insert(name.to_owned(), vec![0]);
        }

        let _ = cache.get("a");

        assert_eq!(cache.keys_oldest_first(), ["b", "c", "a"]);
    }

    #[test]
    fn a_miss_does_not_reorder() {
        let mut cache = Osc99IconCache::default();
        for name in ["a", "b"] {
            cache.insert(name.to_owned(), vec![0]);
        }

        let _ = cache.get("zzz");

        assert_eq!(cache.keys_oldest_first(), ["a", "b"]);
    }

    #[test]
    fn the_least_recently_used_entry_is_evicted_at_the_entry_cap() {
        let mut cache = Osc99IconCache::default();
        for i in 0..MAX_ICON_CACHE_ENTRIES {
            cache.insert(key(i), vec![0]);
        }
        // Touch the oldest so the second oldest becomes the victim.
        let _ = cache.get(&key(0));

        cache.insert("new".to_owned(), vec![0]);

        assert_eq!(cache.len(), MAX_ICON_CACHE_ENTRIES);
        assert_eq!(cache.get(&key(1)), None, "the LRU entry is evicted");
        assert!(cache.get(&key(0)).is_some(), "the touched entry survives");
        assert!(cache.get("new").is_some());
    }

    #[test]
    fn the_entry_count_never_exceeds_the_cap() {
        let mut cache = Osc99IconCache::default();
        for i in 0..500 {
            cache.insert(key(i), vec![0u8; 16]);
            assert!(cache.len() <= MAX_ICON_CACHE_ENTRIES);
        }
        // The survivors are the most recent insertions.
        assert!(cache.get(&key(499)).is_some());
        assert_eq!(cache.get(&key(0)), None);
    }

    #[test]
    fn the_total_bytes_never_exceed_the_budget_and_evict_oldest_first() {
        let mut cache = Osc99IconCache::default();
        let chunk = 256 * 1024;
        for i in 0..64 {
            cache.insert(key(i), vec![0u8; chunk]);
            assert!(cache.total_bytes <= MAX_ICON_CACHE_BYTES);
        }
        // 4 MiB holds exactly 16 chunks: the last 16 insertions survive.
        assert_eq!(cache.len(), MAX_ICON_CACHE_BYTES / chunk);
        assert_eq!(cache.get(&key(47)), None);
        assert!(cache.get(&key(48)).is_some());
        assert!(cache.get(&key(63)).is_some());
    }

    #[test]
    fn an_oversized_entry_is_not_cached_and_removes_the_old_one() {
        let mut cache = Osc99IconCache::default();
        cache.insert("a".to_owned(), vec![1]);
        cache.insert("b".to_owned(), vec![2]);

        cache.insert("a".to_owned(), vec![0u8; MAX_ICON_CACHE_BYTES + 1]);

        assert_eq!(cache.get("a"), None);
        assert_eq!(cache.keys_oldest_first(), ["b"]);
        assert_eq!(cache.total_bytes, 1);
    }

    #[test]
    fn an_entry_of_exactly_the_budget_is_cached() {
        let mut cache = Osc99IconCache::default();
        cache.insert("a".to_owned(), vec![1]);

        cache.insert("big".to_owned(), vec![0u8; MAX_ICON_CACHE_BYTES]);

        assert_eq!(cache.keys_oldest_first(), ["big"]);
        assert_eq!(cache.total_bytes, MAX_ICON_CACHE_BYTES);
    }
}
