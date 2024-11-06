use hickory_proto::rr::{Name, Record, RecordType};
use lru::LruCache;
use std::fmt::Debug;
use std::hash::Hash;
use std::num::NonZeroUsize;
use std::sync::Mutex;
use std::time::{Duration, Instant};
use tracing::instrument;

#[derive(Debug)]
pub(crate) struct Cache<K: Hash + Eq, V> {
    lru: Mutex<LruCache<K, ValueWithTTL<V>>>,
}

struct ValueWithTTL<V> {
    value: V,
    valid_before: Instant,
}

/// This is an LRU cache with TTL support with locking to enable multiple threads getting and
/// storing values.
impl<K: Hash + Eq + Debug, V: Clone + Debug> Cache<K, V> {
    pub(crate) fn new(capacity: NonZeroUsize) -> Cache<K, V> {
        Cache { lru: Mutex::new(LruCache::new(capacity)) }
    }
    #[instrument(name = "cache-store", skip(self))]
    pub(crate) fn store_with_ttl(&self, question: K, value: V, valid_before: Instant) {
        self.lru.lock().unwrap().put(question, ValueWithTTL { value, valid_before });
    }

    #[instrument(name = "cache-get", skip(self), fields(hit = false, expired = false))]
    pub(crate) fn get_with_remaining_ttl(&self, key: &K, now: Instant) -> Option<(V, Duration)> {
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
            Some((with_ttl.value.clone(), with_ttl.valid_before - now))
        }
    }
}

pub(crate) type DnsCache = Cache<Query, Vec<Record>>;

#[derive(Debug, Hash, Eq, PartialEq, Clone)]
pub(crate) struct Query {
    pub to_resolve: Name,
    pub record_type: RecordType,
}

/// Some convenient methods for Caches that holds DNS data
impl DnsCache {
    /// extracts the ttl from the Record to be stored, to make it a bit more ergonomic to use
    pub(crate) fn store(&self, query: Query, value: Vec<Record>, now: Instant) {
        let min_ttl = value.iter().map(Record::ttl).min().unwrap_or(0);
        let min_ttl = Duration::from_secs(min_ttl as u64);
        self.store_with_ttl(query, value, now + min_ttl);
    }

    pub(crate) fn get_and_update_ttl(&self, query: &Query, now: Instant) -> Option<Vec<Record>> {
        self.get_with_remaining_ttl(query, now).map(update_ttl)
    }
}

/// Creates and returns a copy of Vec<Record> replacing the ttl value in each of the records with
/// the passed duration.
fn update_ttl(item: (Vec<Record>, Duration)) -> Vec<Record> {
    item.0
        .iter()
        .map(|r| {
            let mut r = r.clone();
            r.set_ttl(item.1.as_secs() as u32);
            r
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use crate::a;
    use crate::cache::{update_ttl, Cache, DnsCache, Query};
    use hickory_proto::rr::{rdata, RData, Record, RecordType};
    use std::num::NonZeroUsize;
    use std::time::{Duration, Instant};

    #[test]
    fn test_cache() {
        let capacity: NonZeroUsize = NonZeroUsize::new(5).unwrap();
        let cache = &mut Cache::new(capacity);
        let now = Instant::now();
        for i in 0..5 {
            let ttl = now + Duration::from_secs(10);
            cache.store_with_ttl(format!("key{i}"), "value0", ttl);
        }

        let result = cache.get_with_remaining_ttl(&"key0".to_owned(), Instant::now());
        assert!(result.is_some());
        assert_eq!(result.unwrap().0, "value0");
        // verify that the time remaining is close to 10 seconds
        let remaining = result.unwrap().1;
        assert!(Duration::from_secs(10) - remaining < Duration::from_secs(1));

        assert_eq!(cache.lru.lock().unwrap().len(), 5);
        let result =
            cache.get_with_remaining_ttl(&"key1".to_owned(), now + Duration::from_secs(20));
        assert!(result.is_none());
        assert_eq!(cache.lru.lock().unwrap().len(), 4);

        assert!(cache.get_with_remaining_ttl(&"key42".to_owned(), now).is_none());
    }

    #[test]
    fn test_update_ttl() -> anyhow::Result<()> {
        let mut record = a!("example.com", "127.0.0.1");
        record.set_ttl(47);
        let mut another = a!("another.com", "127.0.0.1");
        another.set_ttl(48);

        let result = update_ttl((vec![record, another], Duration::from_secs(42)));
        assert!(result.into_iter().map(|r| r.ttl()).all(|ttl| ttl == 42));
        Ok(())
    }

    #[test]
    fn test_get_and_update_ttl() -> anyhow::Result<()> {
        let mut record = a!("example.com", "127.0.0.1");
        record.set_ttl(47);
        let cache = DnsCache::new(NonZeroUsize::new(1).unwrap());
        let query = Query { to_resolve: "example.com.".parse()?, record_type: RecordType::A };
        let when = Instant::now();
        cache.store(query.clone(), vec![record], when);

        let result = cache.get_and_update_ttl(&query, when + Duration::from_secs(10));
        assert!(result.is_some());
        assert!(result.unwrap().iter().all(|r| r.ttl() == 37));
        Ok(())
    }
}
