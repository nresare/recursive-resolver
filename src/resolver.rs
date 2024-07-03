use crate::backend::{Backend, UdpBackend};
use anyhow::Result;
use hickory_resolver::proto::rr::{Record, RecordType};
use hickory_resolver::Name;
use std::net::IpAddr;

struct RecursiveResolver {
    backend: Box<dyn Backend>,
    roots: Vec<IpAddr>,
}

impl RecursiveResolver {
    fn new() -> Self {
        RecursiveResolver {
            backend: Box::new(UdpBackend::new()),
            roots: vec![
                IpAddr::V4("192.36.148.17".parse().unwrap()),
                IpAddr::V6("2001:7fe::53".parse().unwrap()),
            ],
        }
    }

    async fn resolve(&mut self, name: Name, record_type: RecordType) -> Result<Vec<Record>> {
        Err(anyhow::Error::msg("not here yet"))
    }
}

#[cfg(test)]
mod test {
    use crate::resolver::RecursiveResolver;
    use anyhow::Result;
    use hickory_resolver::proto::rr::RecordType;
    use hickory_resolver::Name;
    use std::str::FromStr;

    #[tokio::test]
    async fn test_resolve() -> Result<()> {
        let mut resolver = RecursiveResolver::new();
        let _ = resolver
            .resolve(Name::from_str("noa.re.")?, RecordType::A)
            .await?;
        Ok(())
    }
}
