use lru::LruCache;
use std::fmt::Debug;
use std::hash::Hash;
use std::num::NonZeroUsize;
use std::sync::Mutex;
use std::time::Instant;
use tracing::instrument;

#[derive(Debug)]
pub(crate) struct Cache<K: Hash + Eq, V> {
    lru: Mutex<LruCache<K, ValueWithTTL<V>>>,
}

struct ValueWithTTL<V> {
    value: V,
    valid_before: Instant,
}

impl<K: Hash + Eq + Debug, V: Clone> Cache<K, V> {
    pub(crate) fn new(capacity: NonZeroUsize) -> Cache<K, V> {
        Cache { lru: Mutex::new(LruCache::new(capacity)) }
    }
    pub(crate) fn store(&self, question: K, value: V, valid_before: Instant) {
        self.lru.lock().unwrap().put(question, ValueWithTTL { value, valid_before });
    }

    #[instrument(name = "cache-get", skip(self), fields(hit = false, expired = false))]
    pub(crate) fn get(&self, key: &K, now: Instant) -> Option<V> {
        let mut guard = self.lru.lock().unwrap();
        let span = tracing::Span::current();
        let with_ttl = guard.get(key)?;
        if with_ttl.valid_before < now {
            // the value has expired, remove it
            guard.pop(key);
            span.record("expired", true);
            None
        } else {
            span.record("hit", true);
            Some(with_ttl.value.clone())
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::cache::Cache;
    use std::num::NonZeroUsize;
    use std::time::{Duration, Instant};

    #[test]
    fn test_cache() {
        let capacity: NonZeroUsize = NonZeroUsize::new(5).unwrap();
        let cache = &mut Cache::new(capacity);
        let now = Instant::now();
        for i in 0..5 {
            let ttl = now + Duration::from_secs(10);
            cache.store(format!("key{i}"), "value0", ttl);
        }

        let result = cache.get(&"key0".to_owned(), Instant::now());
        assert!(result.is_some());
        assert_eq!(result.unwrap(), "value0");

        assert_eq!(cache.lru.lock().unwrap().len(), 5);
        let result = cache.get(&"key1".to_owned(), now + Duration::from_secs(20));
        assert!(result.is_none());
        assert_eq!(cache.lru.lock().unwrap().len(), 4);

        assert!(cache.get(&"key42".to_owned(), now).is_none());
    }
}
