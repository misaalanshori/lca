//! One bounded, deterministic cache policy for the whole engine (gh #323).
//!
//! # Why a shared type rather than a third hand-rolled map
//!
//! Three caches existed before this module and each evicted differently:
//! `text.rs`'s width cache and `widgets/mermaid.rs`'s parse cache both removed
//! `keys().next()` — and `HashMap` iteration order is hash order, so that is
//! **arbitrary**, not oldest. A hot width computed once could be dropped by the
//! next cold insert, which is a cache that misses exactly when it is working
//! hardest. Two hand-rolled copies of one wrong idea is the argument for one
//! type; the brief asks for one policy in the codebase and this is it.
//!
//! # The policy: least-recently-used, and why not insertion order
//!
//! LRU is the honest default for a *width* cache: the strings a frame measures
//! are the ones it will measure again next frame, so "recently used" predicts
//! "used soon" far better than "recently inserted". A tree's worth of new rows
//! scrolls in while the visible rows stay the same, and insertion-order eviction
//! would push out the visible ones first.
//!
//! Tie-breaking is deterministic. Entries carry a monotonically increasing stamp
//! incremented on every insert *and* every hit, so an eviction always names the
//! single least-recently-used entry and two runs on the same input sequence evict
//! the same keys. That is what makes the behaviour testable at all: "arbitrary"
//! has no row that can fail.
//!
//! # Cost
//!
//! A hit touches the entry's stamp, which means a `get` under a mutable borrow.
//! That is one `i64` store on a path that already hashes the key, and it is what
//! LRU *is* — an eviction policy that does not track access is not one. The
//! alternative (a `VecDeque` of keys) trades the store for an O(n) position scan,
//! which is strictly worse at these sizes.
//!
//! # The bound is per instance and the caller picks it
//!
//! [`LruCache::new`] takes the capacity. There is no default, because the right
//! bound depends on what is being cached (512 widths, 20 parsed diagrams) and a
//! default would invite a caller to inherit one that does not fit.

use std::collections::HashMap;
use std::hash::Hash;

/// A bounded map that evicts the least-recently-used entry when full.
///
/// See the module docs for the policy and why it is shared.
#[derive(Debug)]
pub struct LruCache<K, V> {
    entries: HashMap<K, Entry<V>>,
    /// Monotonic, never reset, so a stored stamp is comparable across evictions.
    clock: u64,
    capacity: usize,
}

#[derive(Debug)]
struct Entry<V> {
    value: V,
    /// When this entry was last inserted or hit. Higher is more recent.
    stamp: u64,
}

impl<K: Eq + Hash + Clone, V: Clone> LruCache<K, V> {
    /// A cache holding at most `capacity` entries.
    ///
    /// A capacity of zero is clamped to one: a cache that can hold nothing would
    /// insert and immediately evict on every call, which is a slower way of not
    /// having a cache, and a caller that passed zero meant "smallest", not
    /// "disabled".
    #[must_use]
    pub fn new(capacity: usize) -> LruCache<K, V> {
        LruCache {
            entries: HashMap::new(),
            clock: 0,
            capacity: capacity.max(1),
        }
    }

    /// The value for `key`, marking it most-recently-used.
    pub fn get(&mut self, key: &K) -> Option<V> {
        // Two steps so the borrow of `entries` and the write to `clock` do not
        // overlap: read the stamp, then bump both.
        let stamp = self.entries.get(key).map(|entry| entry.stamp)?;
        self.clock = self.clock.wrapping_add(1);
        let now = self.clock;
        if let Some(entry) = self.entries.get_mut(key) {
            entry.stamp = now;
            return Some(entry.value.clone());
        }
        // Unreachable (the `get` above found it and nothing ran between), but a
        // `None` here is the honest answer rather than a panic in a cache.
        let _ = stamp;
        None
    }

    /// Insert `key`, evicting the least-recently-used entry when full.
    ///
    /// Re-inserting a present key replaces its value and refreshes its stamp,
    /// which is what a caller that recomputed it means.
    pub fn insert(&mut self, key: K, value: V) {
        self.clock = self.clock.wrapping_add(1);
        let now = self.clock;
        if !self.entries.contains_key(&key) && self.entries.len() >= self.capacity {
            self.evict_least_recent();
        }
        self.entries.insert(key, Entry { value, stamp: now });
    }

    /// How many entries are held (never more than the capacity).
    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether the cache holds nothing.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// The configured capacity.
    #[must_use]
    pub fn capacity(&self) -> usize {
        self.capacity
    }

    /// Whether `key` is present, without touching its recency.
    ///
    /// For assertions and diagnostics: a hit here must not change what the next
    /// eviction picks, or a test observing the cache would change it.
    #[must_use]
    pub fn contains(&self, key: &K) -> bool {
        self.entries.contains_key(key)
    }

    /// Drop the entry with the lowest stamp.
    fn evict_least_recent(&mut self) {
        let Some(victim) = self
            .entries
            .iter()
            .min_by_key(|(_, entry)| entry.stamp)
            .map(|(key, _)| key.clone())
        else {
            return;
        };
        self.entries.remove(&victim);
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code

    use super::*;

    // Verifies: gh #323 defect 1 - a hot entry survives a flood of cold inserts.
    //
    // This is the row the issue exists for. Under `HashMap::keys().next()` the
    // victim is hash order, so one of these cold inserts eventually takes the hot
    // key and the cache starts thrashing exactly when it is working hardest.
    #[test]
    fn a_hot_entry_is_not_evicted_by_cold_inserts() {
        let mut cache: LruCache<String, usize> = LruCache::new(4);
        cache.insert("hot".to_string(), 1);
        for index in 0..1000 {
            // Re-use the hot key every round, the way a frame re-measures its
            // visible rows while new ones scroll in.
            assert_eq!(cache.get(&"hot".to_string()), Some(1), "round {index}");
            cache.insert(format!("cold{index}"), index);
        }
        assert_eq!(
            cache.get(&"hot".to_string()),
            Some(1),
            "the hot entry survives"
        );
    }

    // Verifies: the policy is deterministic - the same sequence evicts the same
    // keys, so an eviction is a decision rather than an accident of hashing.
    #[test]
    fn the_same_sequence_evicts_the_same_entries_every_time() {
        let run = || {
            let mut cache: LruCache<String, usize> = LruCache::new(3);
            for index in 0..10 {
                cache.insert(format!("k{index}"), index);
                // Vary the access pattern so recency is not just insertion order.
                let _ = cache.get(&format!("k{}", index / 2));
            }
            (0..10)
                .filter(|index| cache.contains(&format!("k{index}")))
                .collect::<Vec<_>>()
        };
        assert_eq!(run(), run(), "identical input, identical survivors");
    }

    // Verifies: the victim really is the *least recently used*, not the oldest
    // inserted and not an arbitrary one.
    //
    // The sequence is built so all three policies disagree: `a` is inserted first
    // but touched last, so LRU keeps it while FIFO would drop it.
    #[test]
    fn the_victim_is_the_least_recently_used_not_the_oldest() {
        let mut cache: LruCache<String, usize> = LruCache::new(2);
        cache.insert("a".to_string(), 1); // oldest by insertion
        cache.insert("b".to_string(), 2);
        assert_eq!(cache.get(&"a".to_string()), Some(1)); // ...but most recent by use
        cache.insert("c".to_string(), 3); // must evict `b`, not `a`
        assert!(
            cache.contains(&"a".to_string()),
            "the recently used entry stays"
        );
        assert!(!cache.contains(&"b".to_string()), "the stale one goes");
        assert!(cache.contains(&"c".to_string()));
    }

    // Verifies: the bound holds. A cache that grows past its capacity is the bug
    // it was meant to prevent, wearing a friendlier name.
    #[test]
    fn the_cache_never_exceeds_its_capacity() {
        let mut cache: LruCache<String, usize> = LruCache::new(8);
        for index in 0..500 {
            cache.insert(format!("k{index}"), index);
            assert!(cache.len() <= 8, "len {} at {index}", cache.len());
        }
        assert_eq!(cache.len(), 8);
        assert_eq!(cache.capacity(), 8);
    }

    // Verifies: re-inserting a present key refreshes it rather than counting
    // twice, so a recomputed value does not silently consume a second slot.
    #[test]
    fn reinserting_a_present_key_replaces_and_refreshes() {
        let mut cache: LruCache<String, usize> = LruCache::new(2);
        cache.insert("a".to_string(), 1);
        cache.insert("b".to_string(), 2);
        cache.insert("a".to_string(), 10); // replace, refresh
        assert_eq!(cache.len(), 2, "no third slot was taken");
        assert_eq!(cache.get(&"a".to_string()), Some(10));
        cache.insert("c".to_string(), 3); // evicts `b`, the staler of the two
        assert!(cache.contains(&"a".to_string()));
        assert!(!cache.contains(&"b".to_string()));
    }

    // Verifies: `get` marks an entry used, which is the whole difference between
    // this and the insertion-order map it replaced.
    #[test]
    fn a_get_makes_an_entry_recent() {
        let mut cache: LruCache<String, usize> = LruCache::new(2);
        cache.insert("a".to_string(), 1);
        cache.insert("b".to_string(), 2);
        // Touch `a`, making `b` the least recent.
        let _ = cache.get(&"a".to_string());
        cache.insert("c".to_string(), 3);
        assert!(cache.contains(&"a".to_string()), "touched, so it stays");
        assert!(!cache.contains(&"b".to_string()), "untouched, so it goes");
    }

    // Verifies: a zero capacity is clamped rather than panicking or looping, so a
    // caller cannot wedge the cache by asking for nothing.
    #[test]
    fn a_zero_capacity_is_clamped_to_one() {
        let mut cache: LruCache<String, usize> = LruCache::new(0);
        assert_eq!(cache.capacity(), 1);
        cache.insert("a".to_string(), 1);
        cache.insert("b".to_string(), 2);
        assert_eq!(cache.len(), 1, "still bounded");
        assert!(cache.contains(&"b".to_string()), "the newest survives");
    }

    // Verifies: an absent key answers `None` and changes nothing, so a miss does
    // not perturb eviction order.
    #[test]
    fn a_miss_leaves_the_cache_untouched() {
        let mut cache: LruCache<String, usize> = LruCache::new(2);
        cache.insert("a".to_string(), 1);
        assert_eq!(cache.get(&"nope".to_string()), None);
        assert!(!cache.is_empty());
        assert!(cache.contains(&"a".to_string()));
    }
}
