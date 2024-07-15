use std::net::IpAddr;

use anyhow::Result;
use async_trait::async_trait;
use hickory_resolver::proto::rr::{RData, Record, RecordType};
use hickory_resolver::Name;
use rand::seq::SliceRandom;
use rand::thread_rng;

use crate::resolver::RecursiveResolver;

#[async_trait]
pub trait IpProvider {
    async fn next(&mut self) -> Result<Option<IpAddr>>;
}

pub(crate) struct RootsProvider<'a> {
    shuffled_pointers: Vec<&'a IpAddr>,
}

impl<'a> RootsProvider<'a> {
    pub(crate) fn new(roots: &'a Vec<IpAddr>) -> Self {
        let mut shuffled_pointers: Vec<&IpAddr> = roots.iter().collect();
        shuffled_pointers.shuffle(&mut thread_rng());
        RootsProvider { shuffled_pointers }
    }
}

#[async_trait]
impl IpProvider for RootsProvider<'_> {
    async fn next(&mut self) -> Result<Option<IpAddr>> {
        Ok(self.shuffled_pointers.pop().map(|r| r.clone()))
    }
}

pub(crate) struct NsProvider<'a> {
    shuffled_nameservers: Vec<Record>,
    glue: Vec<Record>,
    resolver: &'a RecursiveResolver,
}

impl<'a> NsProvider<'a> {
    pub(crate) fn new(
        nameservers: Vec<Record>,
        glue: Vec<Record>,
        resolver: &'a RecursiveResolver,
    ) -> Self {
        let mut shuffled_nameservers = nameservers.clone();
        shuffled_nameservers.shuffle(&mut thread_rng());
        NsProvider {
            shuffled_nameservers,
            glue,
            resolver,
        }
    }
    // todo: return all the records, lookup both A and AAAA
    async fn get_ip(&self, ns: &Record, glue: &Vec<Record>) -> Result<IpAddr> {
        let name = get_ns_name(ns)?;
        if let Some(ip) = find_in_glue(name, glue) {
            return Ok(ip);
        }
        let mut result = self.resolver.resolve(name, RecordType::A).await?;
        match result.pop() {
            None => Err(anyhow::Error::msg("unexpected empty result")),
            Some(record) => match record.data() {
                Some(RData::A(a)) => Ok(IpAddr::V4(a.0.clone())),
                _ => Err(anyhow::Error::msg("no rdata, or wrong type of rdata")),
            },
        }
    }
}
#[async_trait]
impl<'a> IpProvider for NsProvider<'a> {
    async fn next(&mut self) -> Result<Option<IpAddr>> {
        match self.shuffled_nameservers.pop() {
            None => Ok(None),
            Some(ns) => Ok(Some(self.get_ip(&ns, &self.glue).await?)),
        }
    }
}

fn find_in_glue(name: &Name, glue: &[Record]) -> Option<IpAddr> {
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
        return Err(anyhow::Error::msg("Record type not NS"));
    }
    match record.data() {
        Some(RData::NS(ns)) => Ok(&ns.0),
        Some(_) => Err(anyhow::Error::msg("wrong type of rdata")),
        _ => Err(anyhow::Error::msg("No rdata")),
    }
}

#[cfg(test)]
mod tests {
    use std::str::FromStr;

    use anyhow::Result;
    use hickory_proto::rr::rdata::a::A;
    use hickory_resolver::proto::rr::{RData, Record};
    use hickory_resolver::IntoName;

    use crate::selector::find_in_glue;

    #[test]
    fn test_find_in_glue() -> Result<()> {
        let ip0 = "172.104.148.31";
        let glue = vec![
            record("ns0.resare.com", ip0)?,
            record("ns1.resare.com", "140.238.85.157")?,
        ];
        let result = find_in_glue(&"ns0.resare.com".into_name()?, &glue);
        assert_eq!(Some(ip0.parse()?), result);
        Ok(())
    }

    fn record(name: impl IntoName, ipv4_addr: &str) -> Result<Record> {
        Ok(Record::from_rdata(
            name.into_name()?,
            0,
            RData::A(A::from_str(ipv4_addr)?),
        ))
    }
}
