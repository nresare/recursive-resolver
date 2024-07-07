use std::net::IpAddr;

use anyhow::Result;
use async_trait::async_trait;
use hickory_resolver::Name;
use hickory_resolver::proto::rr::{RData, Record, RecordType};
use rand::seq::SliceRandom;
use rand::thread_rng;

use crate::resolver::RecursiveResolver;

#[async_trait]
trait IpProvider {
    async fn next(&mut self) -> Result<Option<IpAddr>>;
}

pub(crate) struct RootsProvider<'a> {
    shuffled_pointers: Vec<&'a IpAddr>,
}

impl <'a> RootsProvider<'a> {
    pub(crate) fn new(roots: &'a Vec<IpAddr>) -> Self {
        let mut shuffled_pointers: Vec<&IpAddr> = roots.iter().collect();
        shuffled_pointers.shuffle(&mut thread_rng());
        RootsProvider { shuffled_pointers }
    }
}

impl Iterator for RootsProvider<'_> {
    type Item = IpAddr;

    fn next(&mut self) -> Option<Self::Item> {
        self.shuffled_pointers.pop().map(|r| r.clone())
    }
}

struct NsProvider<'a> {
    nameservers: &'a Vec<Record>,
    glue: &'a Vec<Record>,
    shuffled_nameservers: Vec<&'a Record>,
    resolver: &'a RecursiveResolver,
}

impl <'a>NsProvider<'a> {
    fn new(nameservers: &'a Vec<Record>, glue: &'a Vec<Record>, resolver: &'a RecursiveResolver) -> Self {
        let mut shuffled_nameservers: Vec<&Record> = nameservers.iter().collect();
        shuffled_nameservers.shuffle(&mut thread_rng());
        NsProvider {
            nameservers,
            glue,
            shuffled_nameservers,
            resolver
        }
    }
    // todo: return all the records, lookup both A and AAAA
    async fn get_ip(&self, ns: &Record, glue: &Vec<Record>) -> Result<IpAddr> {
        let name = get_ns_name(ns)?;
        if let Some(ip) = find_in_glue(name, glue) {
            return Ok(ip)
        }
        let mut result = self.resolver.resolve(name, RecordType::A).await?;
        match result.pop() {
            None => Err(anyhow::Error::msg("unexpected empty result")),
            Some(record) => {
                match record.data() {
                    Some(RData::A(a)) => Ok(IpAddr::V4(a.0.clone())),
                    _ => Err(anyhow::Error::msg("no rdata, or wrong type of rdata"))
                }
            }
        }
    }
}
#[async_trait]
impl <'a>IpProvider for NsProvider<'a> {
    async fn next(&mut self) -> Result<Option<IpAddr>> {
        match self.shuffled_nameservers.pop() {
            None => Ok(None),
            Some(ns) => {
                Ok(Some(self.get_ip(ns, self.glue).await?))
            },
        }
    }
}

fn find_in_glue(name: &Name, glue: &Vec<Record>) -> Option<IpAddr> {
     glue.iter()
        .filter(|r| r.record_type() == RecordType::A)
        .filter(|r| r.name() == name)
        .filter_map(|r| {
            if let Some(&RData::A(a)) = r.data() {
                Some(IpAddr::V4(a.0))
            } else {
                None
            }
        })
        .next()
}

fn get_ns_name(record: &Record) -> Result<&Name> {
    if record.record_type() != RecordType::NS {
        return Err(anyhow::Error::msg("Record type not NS"))
    }
    match record.data() {
        Some(RData::NS(ns)) => {
            Ok(&ns.0)
        },
        Some(_) => Err(anyhow::Error::msg("wrong type of rdata")),
        _ => Err(anyhow::Error::msg("No rdata")),
    }
}



#[cfg(test)]
mod tests {
    use std::net::{IpAddr, Ipv4Addr};
    use hickory_resolver::IntoName;
    use hickory_resolver::proto::rr::{RData, Record};
    use crate::selector::{find_in_glue, RootsProvider};
    use anyhow::Result;

    use hickory_proto::rr::rdata::a::A;

    #[test]
    fn test_iterate() {
        let addrs = vec![
            IpAddr::V4(Ipv4Addr::new(127,0, 0, 1)),
            IpAddr::V4(Ipv4Addr::new(127,0, 0, 2)),
        ];

        let provider = RootsProvider::new(&addrs);
        for ip in provider {
            println!("{}", ip)
        }
    }

    #[test]
    fn test_find_in_glue() -> Result<()> {
        let glue = vec![
            record("ns0.resare.com", "172.104.148.31")?,
            record("ns1.resare.com", "140.238.85.157")?,
        ];



        Ok(())
    }

    fn record(name: impl IntoName, ipv4_addr: &str) -> Result<Record> {
        let addr: Ipv4Addr = ipv4_addr.into();
        let mut r: Record = Record::new();
        r.set_name(name.into_name()?);
        r.set_data(Some(RData::A(A(Ipv4Addr::V4(addr)))));
        Ok(r)
    }
}