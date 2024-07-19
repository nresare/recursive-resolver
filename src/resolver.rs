use std::fmt::Debug;
use std::net::IpAddr;

use anyhow::Result;
use hickory_resolver::proto::op::{Message, ResponseCode};
use hickory_resolver::proto::rr::{Record, RecordType};
use hickory_resolver::Name;
use tracing::{debug, instrument};

use crate::backend::{Backend, UdpBackend};
use crate::resolver::QueryResponse::{Answer, Referral};
use crate::selector::{IpProvider, NsProvider, RootsProvider};

#[derive(Debug)]
pub struct RecursiveResolver {
    backend: Box<dyn Backend + Sync + Send>,
    roots: Vec<IpAddr>,
}

impl RecursiveResolver {
    pub fn new() -> Self {
        RecursiveResolver {
            backend: Box::new(UdpBackend::new()),
            roots: vec![
                IpAddr::V4("192.36.148.17".parse().unwrap()),
                //IpAddr::V6("2001:7fe::53".parse().unwrap()),
            ],
        }
    }

    #[cfg(test)]
    fn from_backend(backend: impl Backend + Send + Sync + 'static, roots: Vec<IpAddr>) -> Self {
        RecursiveResolver {
            backend: Box::new(backend),
            roots,
        }
    }

    #[instrument]
    pub async fn resolve(
        &self,
        name: &Name,
        record_type: RecordType,
    ) -> Result<Vec<Record>> {
        debug!(hostname = format!("{}", name), "Resolving");
        let mut candidates: Box<dyn IpProvider + Send> = Box::new(RootsProvider::new(&self.roots));
        loop {
            let target = candidates
                .next()
                .await?
                .ok_or_else(|| anyhow::Error::msg("no more ns's to try"))?;
            debug!(target = format!("{}", &target), "Contacting");
            let response = self.resolve_inner(target, name, record_type).await;
            match response {
                QueryResponse::Failure(e) => return Err(e),
                QueryResponse::NxDomain => todo!(),
                Referral(ns, glue) => {
                    debug!(?ns, "Received a redirect");
                    candidates = Box::new(NsProvider::new(ns, glue, self))
                }
                Answer(answers) => return Ok(answers),
            }
        }
    }

    async fn resolve_inner(
        &self,
        target: IpAddr,
        name: &Name,
        record_type: RecordType,
    ) -> QueryResponse {
        match self.backend.query(target, name, record_type).await {
            Err(e) => QueryResponse::Failure(e),
            Ok(message) => {
                if message.response_code() == ResponseCode::NXDomain {
                    QueryResponse::NxDomain
                } else if is_final(&message) {
                    Answer(message.answers().to_vec())
                } else {
                    Referral(
                        message.name_servers().to_vec(),
                        message.additionals().to_vec(),
                    )
                }
            }
        }
    }
}

enum QueryResponse {
    /// The Query failed
    Failure(anyhow::Error),
    /// The domain does not exist
    NxDomain,
    /// There was a response, but the queried server was not authoritative for the
    /// name, and returned some Authority records and potentially also Glue records
    Referral(Vec<Record>, Vec<Record>),
    /// There was an authoritative response with answer records
    Answer(Vec<Record>),
}

fn is_final(answer: &Message) -> bool {
    answer.header().authoritative() && !answer.answers().is_empty()
}

#[cfg(test)]
mod test {
    use std::net::{IpAddr, Ipv6Addr};

    use anyhow::Result;
    use hickory_proto::rr::rdata;
    use hickory_proto::rr::{Name, RData, RecordType};
    use hickory_resolver::proto::op::{Header, Message};
    use hickory_resolver::proto::rr::Record;
    use tracing::Level;
    use tracing_subscriber::FmtSubscriber;
    use RecordType::{A, AAAA};

    use crate::fake_backend::FakeBackend;
    use crate::resolver::{is_final, RecursiveResolver};

    #[test]
    fn test_is_final() {
        // not authoritative, no answer
        let m = Message::default();
        assert!(!is_final(&m));

        // authoritative, no answer
        let mut m = Message::new();
        m.set_header(*Header::new().set_authoritative(true));
        assert!(!is_final(&m));

        // not authoritative, answer
        m.set_header(Header::new());
        m.add_answer(Record::new());
        assert!(!is_final(&m));

        m.set_header(*Header::new().set_authoritative(true));
        assert!(is_final(&m));
    }

    macro_rules! ns {
        ($name:expr, $target:expr) => {
            Record::from_rdata($name.parse()?, 0, RData::NS(rdata::NS($target.parse()?)))
        };
    }

    macro_rules! a {
        ($name:expr, $target:expr) => {
            Record::from_rdata($name.parse()?, 0, RData::A(rdata::A(($target.parse()?))))
        };
    }

    macro_rules! aaaa {
        ($name:expr, $target:expr) => {
            Record::from_rdata(
                $name.parse()?,
                0,
                RData::AAAA(rdata::AAAA($target.parse()?)),
            )
        };
    }

    macro_rules! referral {
        ($nameservers:expr) => {{
            let mut msg = Message::new();
            msg.insert_name_servers(vec![$nameservers]);
            msg
        }};
        ($nameservers:expr, $glue:expr) => {{
            let mut msg = Message::new();
            msg.insert_name_servers(vec![$nameservers]);
            msg.insert_additionals(vec![$glue]);
            msg
        }};
    }

    macro_rules! answer {
        ($record:expr) => {{
            let mut msg = Message::new();
            let mut header = Header::default();
            header.set_authoritative(true);
            msg.set_header(header);
            msg.insert_answers(vec![$record]);
            msg
        }};
    }

    #[tokio::test]
    async fn test_resolve() -> Result<()> {
        let mut backend = FakeBackend::new();
        backend.add(
            "10.0.0.1",
            "noa.re",
            AAAA,
            referral!(ns!("re", "ns.nic.fr"), a!("ns.nic.fr", "10.0.0.2")),
        )?;

        backend.add(
            "10.0.0.2",
            "noa.re",
            AAAA,
            referral!(ns!["noa.re", "ns0.resare.com"]),
        )?;

        backend.add(
            "10.0.0.1",
            "ns0.resare.com",
            A,
            referral!(
                ns!("resare.com", "ns0.resare.com"),
                a!("ns0.resare.com", "10.0.0.3")
            ),
        )?;

        // todo: once using glue records is smarter, remove this
        backend.add(
            "10.0.0.3",
            "ns0.resare.com",
            A,
            answer!(a!("ns0.resare.com", "10.0.0.3")),
        )?;

        backend.add("10.0.0.3", "noa.re", AAAA, answer!(aaaa!("noa.re", "::42")))?;

        let resolver =
            RecursiveResolver::from_backend(backend, vec![IpAddr::V4("10.0.0.1".parse()?)]);

        let result = resolver.resolve(&"noa.re".parse()?, AAAA).await?;
        let record = result.first().expect("Could not find record in response");
        assert_eq!(*record.name(), "noa.re".parse::<Name>()?);
        if let Some(RData::AAAA(rdata::AAAA(addr))) = record.data() {
            assert_eq!(*addr, "::42".parse::<Ipv6Addr>()?)
        } else {
            panic!("Could not find AAAA record in result")
        }

        Ok(())
    }

    #[ctor::ctor]
    fn init() {
        let subscriber = FmtSubscriber::builder()
            .with_max_level(Level::DEBUG)
            .finish();
        tracing::subscriber::set_global_default(subscriber)
            .expect("Could not set global default tracing subscriber");
    }
}
