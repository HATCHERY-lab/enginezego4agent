// Portions derived from pi-durable 1.5.2 (packages/durable/src/session/cached-storage.ts), MIT, © 2025 Mario Zechner / Earendil Works. See LICENSES/pi-MIT.txt.
// derived from pi-durable 1.5.2 session/cached-storage, MIT, see LICENSES/pi-MIT.txt.

//! Count-bounded hot-record cache. The least recently used id leaves first.
//! A record bigger than the byte limit is not stored. Pure: no storage, no I/O.

use std::collections::HashMap;

/// Records keyed by id, capped by count. Not a store.
///
/// Callers skip [`Cache::insert`] when [`admits`] is false. `cap` 0 keeps nothing.
pub struct Cache<T: Clone> {
    entries: HashMap<i64, T>,
    /// Oldest at index 0, newest at the end.
    order: Vec<i64>,
    cap: usize,
}

impl<T: Clone> Cache<T> {
    /// Empty cache that keeps at most `cap` records.
    pub fn new(cap: usize) -> Self {
        Self {
            entries: HashMap::new(),
            order: Vec::new(),
            cap,
        }
    }

    /// Store `value` at `id` as the newest record, then drop oldest ids past `cap`.
    pub fn insert(&mut self, id: i64, value: T) {
        self.remove_entry(id);
        self.order.push(id);
        self.entries.insert(id, value);
        self.evict_over_cap();
    }

    /// Clone of the record at `id`, refreshed as newest. `None` when it is absent.
    pub fn get(&mut self, id: i64) -> Option<T> {
        let value = self.entries.get(&id)?.clone();
        self.touch(id);
        Some(value)
    }

    /// Drop `id` when it is present.
    pub fn invalidate(&mut self, id: i64) {
        self.remove_entry(id);
    }

    /// Drop every record.
    pub fn clear(&mut self) {
        self.entries.clear();
        self.order.clear();
    }

    fn touch(&mut self, id: i64) {
        if let Some(pos) = self.order.iter().position(|existing| *existing == id) {
            self.order.remove(pos);
        }
        self.order.push(id);
    }

    fn remove_entry(&mut self, id: i64) {
        self.entries.remove(&id);
        if let Some(pos) = self.order.iter().position(|existing| *existing == id) {
            self.order.remove(pos);
        }
    }

    fn evict_over_cap(&mut self) {
        let victims = evict_oldest(&self.order, self.entries.len(), self.cap);
        let drop_count = victims.len();
        for id in victims {
            self.entries.remove(&id);
        }
        self.order.drain(..drop_count);
    }
}

/// Whether a value of `byte_len` may be stored under `limit`.
///
/// A value larger than `limit` is not stored. A `limit` of 0 stores nothing.
pub fn admits(byte_len: usize, limit: usize) -> bool {
    limit > 0 && byte_len <= limit
}

/// Prefix of `order` to drop so `live - dropped <= cap`.
///
/// `order` is oldest at the front. Empty when `live <= cap`.
/// When `order` is shorter than `live - cap`, the whole prefix is returned.
pub fn evict_oldest(order: &[i64], live: usize, cap: usize) -> Vec<i64> {
    if live <= cap {
        return Vec::new();
    }
    let drop_count = (live - cap).min(order.len());
    order[..drop_count].to_vec()
}

#[cfg(test)]
mod tests {
    use super::{admits, evict_oldest, Cache};

    #[test]
    fn admits_rejects_over_limit_and_a_zero_limit() {
        assert!(admits(0, 1));
        assert!(admits(4, 4));
        assert!(!admits(5, 4));
        assert!(!admits(0, 0));
        assert!(!admits(1, 0));
        assert!(admits(usize::MAX, usize::MAX));
        assert!(!admits(usize::MAX, usize::MAX - 1));
    }

    #[test]
    fn evict_oldest_drops_the_front_prefix() {
        assert_eq!(evict_oldest(&[], 0, 0), Vec::<i64>::new());
        assert_eq!(evict_oldest(&[1, 2, 3], 3, 3), Vec::<i64>::new());
        assert_eq!(evict_oldest(&[1, 2, 3], 2, 5), Vec::<i64>::new());
        assert_eq!(evict_oldest(&[1, 2, 3, 4], 4, 2), vec![1, 2]);
        assert_eq!(evict_oldest(&[10, 20, 30, 40], 4, 0), vec![10, 20, 30, 40]);
        assert_eq!(evict_oldest(&[1, 2, 3, 4], 5, 3), vec![1, 2]);
        assert_eq!(evict_oldest(&[7], 4, 1), vec![7]);
        assert_eq!(evict_oldest(&[], 4, 1), Vec::<i64>::new());
        assert_eq!(evict_oldest(&[-5, -1, 4], 3, 1), vec![-5, -1]);
        assert_eq!(evict_oldest(&[1, 2, 3, 4, 5], 3, 1), vec![1, 2]);
    }

    #[test]
    fn insert_get_replaces_and_refreshes() {
        let mut cache = Cache::new(2);
        cache.insert(1, "a".to_owned());
        cache.insert(2, "b".to_owned());
        assert_eq!(cache.get(1).as_deref(), Some("a"));
        cache.insert(1, "aa".to_owned());
        assert_eq!(cache.get(1).as_deref(), Some("aa"));
        assert_eq!(cache.get(9), None);
    }

    #[test]
    fn oldest_leaves_first_unless_refreshed() {
        let mut cache = Cache::new(2);
        cache.insert(1, "a".to_owned());
        cache.insert(2, "b".to_owned());
        cache.insert(3, "c".to_owned());
        assert_eq!(cache.get(1), None);
        assert_eq!(cache.get(2).as_deref(), Some("b"));
        assert_eq!(cache.get(3).as_deref(), Some("c"));

        let mut refreshed = Cache::new(2);
        refreshed.insert(1, "a".to_owned());
        refreshed.insert(2, "b".to_owned());
        assert_eq!(refreshed.get(1).as_deref(), Some("a"));
        refreshed.insert(3, "c".to_owned());
        assert_eq!(refreshed.get(2), None);
        assert_eq!(refreshed.get(1).as_deref(), Some("a"));
        assert_eq!(refreshed.get(3).as_deref(), Some("c"));
    }

    #[test]
    fn zero_cap_stores_nothing() {
        let mut cache = Cache::new(0);
        cache.insert(1, "a".to_owned());
        assert_eq!(cache.get(1), None);
    }

    #[test]
    fn invalidate_and_clear_drop_ids() {
        let mut cache = Cache::new(2);
        cache.insert(1, "a".to_owned());
        cache.insert(2, "b".to_owned());
        cache.invalidate(1);
        cache.invalidate(99);
        assert_eq!(cache.get(1), None);
        cache.insert(3, "c".to_owned());
        assert_eq!(cache.get(2).as_deref(), Some("b"));
        assert_eq!(cache.get(3).as_deref(), Some("c"));
        cache.clear();
        assert_eq!(cache.get(2), None);
        assert_eq!(cache.get(3), None);
        cache.insert(4, "d".to_owned());
        cache.insert(5, "e".to_owned());
        cache.insert(6, "f".to_owned());
        assert_eq!(cache.get(4), None);
        assert_eq!(cache.get(6).as_deref(), Some("f"));
    }
}
