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

#[derive(Debug)]
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
        let mut shuffled_nameservers: Vec<Record> =
            nameservers.iter().filter(|r| r.record_type() == RecordType::NS).cloned().collect();
        shuffled_nameservers.shuffle(&mut thread_rng());
        NsProvider { shuffled_nameservers, glue }
    }
}

// todo: return all the records, lookup both A and AAAA
async fn get_target(ns: &Record, glue: &[Record]) -> Result<Target, ResolutionError> {
    let Some(result) = get_name_if_ns(ns) else {
        return Err(ServFail("inconsistent data, NsProvider was fed a non-ns record".into()));
    };
    let name = match result {
        Ok(name) => name,
        Err(e) => return Err(e),
    };
    if let Some(ip) = find_in_glue(name, glue) {
        return Ok(Target::Ip(ip));
    }
    Ok(Target::Name(name.to_owned()))
}

#[async_trait]
impl TargetProvider for NsProvider {
    async fn next(&mut self) -> Result<Option<Target>, ResolutionError> {
        match self.shuffled_nameservers.pop() {
            None => Ok(None),
            Some(ns) => Ok(Some(get_target(&ns, &self.glue).await?)),
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

pub(crate) fn get_name_if_ns(record: &Record) -> Option<Result<&Name, ResolutionError>> {
    if record.record_type() != RecordType::NS {
        return None;
    }
    match record.data() {
        Some(RData::NS(ns)) => Some(Ok(&ns.0)),
        // this should not happen, as the RecordType should always agree with the RData type
        Some(_) => Some(Err(ServFail("inconsistent rdata type".to_string()))),
        _ => Some(Err(ServFail("no rdata".to_string()))),
    }
}

#[cfg(test)]
mod tests {
    use crate::target::{find_in_glue, get_name_if_ns, get_target};
    use crate::{a, name, ns};
    use anyhow::Result;
    use hickory_proto::rr::{rdata, RecordType};
    use hickory_proto::rr::{IntoName, Name, RData, Record};
    use std::str::FromStr;

    #[test]
    fn test_find_in_glue() -> Result<()> {
        let ip0 = "172.104.148.31";
        let glue = vec![a!("ns0.c.d", ip0), a!("ns1.c.d", "140.238.85.157")];
        let result = find_in_glue(&"ns0.c.d".into_name()?, &glue);
        assert_eq!(Some(ip0.parse()?), result);
        Ok(())
    }

    #[test]
    fn test_get_name_if_ns() -> Result<()> {
        assert_eq!(&name!("ns0.com."), get_name_if_ns(&ns!("com.", "ns0.com.")).unwrap()?);
        assert!(get_name_if_ns(&a!("foo.com.", "127.0.0.1")).is_none());
        // create a Record with no rdata
        let r = Record::with(name!("ns0.com"), RecordType::NS, 60);
        // since can't compare errors properly because they might hold io::Error that can't be compared
        // we need to do this in a more low tech way
        assert_eq!(
            "Server failure: no rdata",
            get_name_if_ns(&r).unwrap().unwrap_err().to_string()
        );
        // creating an inconsistent Record here, claiming to be an NS but with A rdata
        let mut r = a!("ns0.com.", "127.0.0.1");
        r.set_rr_type(RecordType::NS);
        assert_eq!(
            "Server failure: inconsistent rdata type",
            get_name_if_ns(&r).unwrap().unwrap_err().to_string()
        );
        Ok(())
    }

    #[tokio::test]
    async fn test_get_target_invalid_input() -> Result<()> {
        // the case where the record is of the wrong type
        let result = get_target(&a!("a.b.", "1.2.3.4"), &Vec::new()).await.unwrap_err();
        assert_eq!(
            "Server failure: inconsistent data, NsProvider was fed a non-ns record",
            result.to_string()
        );
        // the case where the record is of the right type but with the wrong data
        let mut r = a!("ns0.com.", "127.0.0.1");
        r.set_rr_type(RecordType::NS);
        let result = get_target(&r, &Vec::new()).await.unwrap_err();
        assert_eq!("Server failure: inconsistent rdata type", result.to_string());
        Ok(())
    }
}
