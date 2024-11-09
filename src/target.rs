use std::net::IpAddr;

use crate::resolver::ResolutionError;
use crate::resolver::ResolutionError::ServFail;
use async_trait::async_trait;
use hickory_proto::rr::{Name, RData, Record, RecordType};
use rand::seq::SliceRandom;
use rand::thread_rng;

#[async_trait]
pub trait TargetProvider {
    async fn next(&mut self) -> Result<Option<Target>, ResolutionError>;
}

pub enum Target {
    Ip(IpAddr),
    Name(Name),
}

pub(crate) struct RootsProvider<'a> {
    shuffled_pointers: Vec<&'a IpAddr>,
}

impl<'a> RootsProvider<'a> {
    pub(crate) fn new(roots: &'a [IpAddr]) -> Self {
        let mut shuffled_pointers: Vec<&IpAddr> = roots.iter().collect();
        shuffled_pointers.shuffle(&mut thread_rng());
        RootsProvider { shuffled_pointers }
    }
}

#[async_trait]
impl TargetProvider for RootsProvider<'_> {
    async fn next(&mut self) -> Result<Option<Target>, ResolutionError> {
        Ok(self.shuffled_pointers.pop().copied().map(Target::Ip))
    }
}

pub(crate) struct NsProvider {
    shuffled_nameservers: Vec<Record>,
    glue: Vec<Record>,
}

impl NsProvider {
    pub(crate) fn new(nameservers: Vec<Record>, glue: Vec<Record>) -> Self {
        let mut shuffled_nameservers = nameservers.clone();
        shuffled_nameservers.shuffle(&mut thread_rng());
        NsProvider { shuffled_nameservers, glue }
    }
    // todo: return all the records, lookup both A and AAAA
    async fn get_target(&self, ns: &Record, glue: &[Record]) -> Result<Target, ResolutionError> {
        let name = get_ns_name(ns)?;
        if let Some(ip) = find_in_glue(name, glue) {
            return Ok(Target::Ip(ip));
        }
        Ok(Target::Name(name.to_owned()))
    }
}

#[async_trait]
impl TargetProvider for NsProvider {
    async fn next(&mut self) -> Result<Option<Target>, ResolutionError> {
        match self.shuffled_nameservers.pop() {
            None => Ok(None),
            Some(ns) => Ok(Some(self.get_target(&ns, &self.glue).await?)),
        }
    }
}

fn find_in_glue(name: &Name, glue: &[Record]) -> Option<IpAddr> {
    glue.iter()
        .filter(|r| r.record_type() == RecordType::A)
        .filter(|r| r.name() == name)
        .filter_map(
            |r| if let Some(&RData::A(a)) = r.data() { Some(IpAddr::V4(a.0)) } else { None },
        )
        .next()
}

pub(crate) fn get_ns_name(record: &Record) -> Result<&Name, ResolutionError> {
    if record.record_type() != RecordType::NS {
        return Err(ServFail("Record type not NS".to_string()));
    }
    match record.data() {
        Some(RData::NS(ns)) => Ok(&ns.0),
        Some(_) => Err(ServFail("wrong type of rdata".to_string())),
        _ => Err(ServFail("No rdata".to_string())),
    }
}

#[cfg(test)]
mod tests {
    use std::str::FromStr;

    use anyhow::Result;
    use hickory_proto::rr::rdata::a::A;
    use hickory_proto::rr::{IntoName, RData, Record};

    use crate::target::find_in_glue;

    #[test]
    fn test_find_in_glue() -> Result<()> {
        let ip0 = "172.104.148.31";
        let glue = vec![record("ns0.c.d", ip0)?, record("ns1.c.d", "140.238.85.157")?];
        let result = find_in_glue(&"ns0.c.d".into_name()?, &glue);
        assert_eq!(Some(ip0.parse()?), result);
        Ok(())
    }

    fn record(name: impl IntoName, ipv4_addr: &str) -> Result<Record> {
        Ok(Record::from_rdata(name.into_name()?, 0, RData::A(A::from_str(ipv4_addr)?)))
    }
}
