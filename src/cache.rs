use crate::cache::CacheResponse::{Authoritative, Referral};
use crate::target::get_ns_name;
use hickory_proto::rr::{Name, RData, Record, RecordType};
use lru::LruCache;
use std::collections::{HashMap, HashSet};
use std::fmt::Debug;
use std::hash::Hash;
use std::num::NonZeroUsize;
use std::sync::Mutex;
use std::time::{Duration, Instant};
use tracing::{debug, instrument, warn};

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
    pub(crate) fn store_with_ttl(&self, key: K, value: V, valid_before: Instant) {
        self.lru.lock().unwrap().put(key, ValueWithTTL { value, valid_before });
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

#[derive(Debug, PartialEq)]
pub(crate) enum CacheResponse {
    Authoritative(Vec<Record>),
    Referral(Vec<Record>, Vec<Record>),
    None,
}

/// Some convenient methods for Caches that holds DNS data
impl DnsCache {
    /// extracts the ttl from the Record to be stored, to make it a bit more ergonomic to use
    pub(crate) fn store(&self, query: Query, value: Vec<Record>, now: Instant) {
        let min_ttl = value.iter().map(Record::ttl).min().unwrap_or(0);
        if min_ttl == 0 {
            return;
        }
        let min_ttl = Duration::from_secs(min_ttl as u64);
        self.store_with_ttl(query, value, now + min_ttl);
    }

    /// a version of store that will validate referral style responses and
    /// create keys from name_server and glue records
    pub(crate) fn store_referral(
        &self,
        name_servers: Vec<Record>,
        glue: Vec<Record>,
        to_resolve: &Name,
        now: Instant,
    ) {
        if !eligible(&name_servers, &glue, to_resolve) {
            return;
        }
        for (query, records) in make_referral_query(&name_servers) {
            self.store(query, records, now)
        }
        for (query, records) in make_referral_query(&glue) {
            self.store(query, records, now)
        }
    }

    fn get_and_update_ttl(&self, query: &Query, now: Instant) -> Option<Vec<Record>> {
        self.get_with_remaining_ttl(query, now).map(update_ttl)
    }

    pub(crate) fn get_best_record(&self, query: &Query, now: Instant) -> CacheResponse {
        if let Some(records) = self.get_and_update_ttl(query, now) {
            return Authoritative(records);
        }
        for parent in parents(&query.to_resolve) {
            let q = Query { to_resolve: parent, record_type: RecordType::NS };
            if let Some(records) = self.get_and_update_ttl(&q, now) {
                return Referral(records.clone(), self.fetch_glue(&records, now));
            }
        }
        CacheResponse::None
    }

    fn fetch_glue(&self, name_servers: &Vec<Record>, now: Instant) -> Vec<Record> {
        let mut result = Vec::with_capacity(name_servers.len());
        for ns in name_servers {
            if let Ok(name) = get_ns_name(ns) {
                let query = Query { to_resolve: name.clone(), record_type: RecordType::A };
                if let Some(records) = self.get_and_update_ttl(&query, now) {
                    result.extend(records);
                }
            } else {
                warn!(%ns, "Invalid NS record retrieved from cache")
            }
        }
        result
    }
}

fn parents(name: &Name) -> Vec<Name> {
    let mut result = Vec::new();
    // the zero label Name is a special case. Has no parents
    let mut name = name.base_name();
    while name.num_labels() > 0 {
        let another = name.base_name();
        result.push(name);
        name = another
    }
    result
}

fn make_referral_query(records: &Vec<Record>) -> HashMap<Query, Vec<Record>> {
    let mut result = HashMap::new();
    if records.is_empty() {
        return result;
    }
    for record in records {
        let query = Query { to_resolve: record.name().clone(), record_type: record.record_type() };
        result.entry(query).or_insert_with(Vec::new).push(record.clone());
    }
    result
}

/// We can only cache records that are relevant to to_resolve, the name we were querying for.
/// This prevents caching of unrelated records that a malicious or misconfigured name server
/// might be providing in responses. We skip all caching if any of the records are wrong.
fn eligible(name_servers: &Vec<Record>, glue: &Vec<Record>, to_resolve: &Name) -> bool {
    let mut names = HashSet::new();
    for name_server in name_servers {
        if let Some(RData::NS(ns)) = name_server.data() {
            names.insert(ns.0.to_string());
        }

        if !name_server.name().zone_of(to_resolve) {
            debug!(%to_resolve, %name_server, "Received out of zone ns record");
            return false;
        }
    }
    for glue in glue {
        if !names.contains(&glue.name().to_string()) {
            debug!(%glue, "Glue record without matching NS");
            return false;
        }
    }
    true
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
    use crate::cache::CacheResponse::{Authoritative, Referral};
    use crate::cache::{
        eligible, make_referral_query, parents, update_ttl, Cache, DnsCache, Query,
    };
    use crate::{a, ns};
    use anyhow::Result;
    use hickory_proto::rr::{rdata, Name, RData, Record, RecordType};
    use std::collections::HashMap;
    use std::num::NonZeroUsize;
    use std::str::FromStr;
    use std::time::{Duration, Instant};

    macro_rules! query {
        ($name:expr, $record_type:expr) => {
            Query { to_resolve: $name.parse()?, record_type: $record_type }
        };
    }

    macro_rules! name {
        ($name:expr) => {
            Name::from_str($name)?
        };
    }
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
    fn test_update_ttl() -> Result<()> {
        let mut record = a!("example.com", "127.0.0.1");
        record.set_ttl(47);
        let mut another = a!("another.com", "127.0.0.1");
        another.set_ttl(48);

        let result = update_ttl((vec![record, another], Duration::from_secs(42)));
        assert!(result.into_iter().map(|r| r.ttl()).all(|ttl| ttl == 42));
        Ok(())
    }

    #[test]
    fn test_zero_ttl() -> Result<()> {
        let mut record = a!("example.com", "127.0.0.1");
        record.set_ttl(0);
        let cache = DnsCache::new(NonZeroUsize::new(1).unwrap());
        let query = query!("example.com", RecordType::A);

        let when = Instant::now();
        cache.store(query.clone(), vec![record], when);

        assert!(cache.get_and_update_ttl(&query, when).is_none());
        Ok(())
    }

    #[test]
    fn test_get_and_update_ttl() -> Result<()> {
        let mut record = a!("example.com", "127.0.0.1");
        record.set_ttl(47);
        let cache = DnsCache::new(NonZeroUsize::new(1).unwrap());
        let query = query!("example.com", RecordType::A);
        let when = Instant::now();
        cache.store(query.clone(), vec![record], when);

        let result = cache.get_and_update_ttl(&query, when + Duration::from_secs(10));
        assert!(result.is_some());
        assert!(result.unwrap().iter().all(|r| r.ttl() == 37));
        Ok(())
    }

    #[test]
    fn test_eligible() -> Result<()> {
        let to_resolve: Name = "example.com.".parse()?;
        assert!(eligible(&vec![ns!("example.com.", "dns.foo.bar")], &vec![], &to_resolve));
        assert!(eligible(&vec![ns!("com", "dns.foo.bar")], &vec![], &to_resolve));
        assert!(!eligible(&vec![ns!("net", "dns.foo.bar")], &vec![], &to_resolve));

        assert!(eligible(
            &vec![ns!("com", "dns.foo.com")],
            &vec![a!("dns.foo.com", "127.0.0.1")],
            &to_resolve
        ));
        assert!(!eligible(
            &vec![ns!("com", "dns.foo.com")],
            &vec![a!("dns.victim.org", "127.0.0.1")],
            &to_resolve
        ));
        // verify that this is case-insensitive
        assert!(eligible(
            &vec![ns!("com", "dns.FOO.com")],
            &vec![a!("dns.foo.com", "127.0.0.1")],
            &to_resolve
        ));
        Ok(())
    }

    #[test]
    fn test_make_referral_query() -> Result<()> {
        let result = make_referral_query(&vec![ns!("com", "a.com"), ns!("com", "b.com")]);
        assert_eq!(
            HashMap::from([(
                query!("com", RecordType::NS),
                vec![ns!("com", "a.com"), ns!("com", "b.com")]
            )]),
            result
        );
        Ok(())
    }

    #[test]
    fn test_store_referral() -> Result<()> {
        let cache = DnsCache::new(NonZeroUsize::new(3).unwrap());
        cache.store_referral(
            vec![ns!("com", "a.com"), ns!("com", "b.com")],
            vec![a!("a.com", "127.0.0.1"), a!("b.com", "127.0.0.3")],
            &"example.com".parse()?,
            Instant::now(),
        );

        let result = cache.get_and_update_ttl(&query!("com", RecordType::NS), Instant::now());
        assert_eq!(Some(vec![ns!("com", "a.com"), ns!("com", "b.com")]), result);

        let result = cache.get_and_update_ttl(&query!("a.com", RecordType::A), Instant::now());
        assert_eq!(Some(vec![a!("a.com", "127.0.0.1")]), result);
        Ok(())
    }

    #[test]
    fn test_store_referral_empty_glue() -> Result<()> {
        let cache = DnsCache::new(NonZeroUsize::new(3).unwrap());
        cache.store_referral(
            vec![ns!("com", "a.com"), ns!("com", "b.com")],
            vec![],
            &"example.com".parse()?,
            Instant::now(),
        );
        let result = cache.get_and_update_ttl(&query!("com", RecordType::NS), Instant::now());
        assert_eq!(Some(vec![ns!("com", "a.com"), ns!("com", "b.com")]), result);

        Ok(())
    }

    #[test]
    fn test_get_best_record_authoritative() -> Result<()> {
        let cache = DnsCache::new(NonZeroUsize::new(1).unwrap());
        let q = query!("example.com", RecordType::A);
        cache.store(q.clone(), vec![a!("example.com", "127.0.0.1")], Instant::now());
        // direct hit
        let result = cache.get_best_record(&q, Instant::now());
        assert_eq!(Authoritative(vec![a!("example.com", "127.0.0.1")]), result);

        Ok(())
    }

    #[test]
    fn test_get_best_record_referral() -> Result<()> {
        let cache = DnsCache::new(NonZeroUsize::new(3).unwrap());
        cache.store_referral(
            vec![ns!("com.", "a.com."), ns!("com.", "b.com.")],
            vec![a!("a.com.", "127.0.0.1"), a!("b.com.", "127.0.0.3")],
            &"example.com.".parse()?,
            Instant::now(),
        );

        let result = cache.get_and_update_ttl(&query!("com.", RecordType::NS), Instant::now());
        assert_eq!(Some(vec![ns!("com.", "a.com."), ns!("com.", "b.com.")]), result);

        let cache = DnsCache::new(NonZeroUsize::new(100).unwrap());
        cache.store_referral(
            vec![ns!("com.", "ns0.com."), ns!("com.", "ns1.com.")],
            vec![a!["ns0.com.", "127.0.0.1"], a!("ns1.com.", "127.0.0.2")],
            &Name::from_str("foo.com.")?,
            Instant::now(),
        );

        let result = cache.get_best_record(&query!("bar.com", RecordType::A), Instant::now());
        assert_eq!(
            Referral(
                vec![ns!("com", "ns0.com"), ns!("com", "ns1.com")],
                vec![a!["ns0.com", "127.0.0.1"], a!("ns1.com", "127.0.0.2")]
            ),
            result
        );
        Ok(())
    }

    #[test]
    fn test_parents() -> Result<()> {
        assert!(parents(&name!("")).is_empty());

        assert_eq!(vec![name!("b.com"), name!("com")], parents(&name!("a.b.com")));
        assert_eq!(
            vec![name!("b.c.com"), name!("c.com"), name!("com")],
            parents(&name!("a.b.c.com"))
        );
        Ok(())
    }
}
