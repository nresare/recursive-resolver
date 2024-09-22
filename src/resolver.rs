use async_recursion::async_recursion;
use hickory_proto::error::ProtoError;
use hickory_proto::op::{Message, ResponseCode};
use hickory_proto::rr::RecordType::A;
use hickory_proto::rr::{Name, RData, Record, RecordType};
use std::fmt::Debug;
use std::net::IpAddr;
use thiserror::Error;
use tracing::{debug, field::Empty, instrument};

use crate::backend::{Backend, UdpBackend};
use crate::resolver::QueryResponse::{Answer, Referral};
use crate::resolver::ResolutionError::{NxDomain, ServFail};
use crate::target::{NsProvider, RootsProvider, Target, TargetProvider};

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
    pub(crate) fn with_backend(
        backend: impl Backend + Send + Sync + 'static,
        roots: Vec<IpAddr>,
    ) -> Self {
        RecursiveResolver { backend: Box::new(backend), roots }
    }

    #[instrument(fields(otel.kind = "server", otel.status_code = "Ok", otel.status_message = Empty, %to_resolve))]
    pub async fn resolve(
        &self,
        to_resolve: &Name,
        record_type: RecordType,
    ) -> Result<Vec<Record>, ResolutionError> {
        let mut state = ResolutionState::new(self);
        let span = tracing::Span::current();
        let result = state.resolve_inner(to_resolve, record_type, 1).await;
        if let Err(e) = result {
            span.record("otel.status_code", "Error");
            span.record("otel.status_message", e.to_string());
            return Err(e);
        }
        result
    }
}
#[derive(Error, Debug)]
pub enum ResolutionError {
    // RFC 1035 4.1.1 RCODE 3 "Name Error"
    #[error("No data exits for this name and record type")]
    NxDomain,
    #[error("Server failure: {0}")]
    ServFail(String),
    #[error("Failure in underlying io")]
    IOError(#[from] std::io::Error),
    #[error("Protocol error (likely serde related)")]
    ProtocolError(#[from] ProtoError),
}
pub(crate) struct ResolutionState<'a> {
    resolver: &'a RecursiveResolver,
    seen: Vec<(Name, RecordType)>,
}

const MAX_RECURSION_DEPTH: u32 = 5;
impl<'a> ResolutionState<'a> {
    pub(crate) fn new(resolver: &'a RecursiveResolver) -> Self {
        ResolutionState { resolver, seen: Vec::new() }
    }

    #[instrument(skip(self), fields(%to_resolve))]
    #[async_recursion]
    async fn resolve_inner(
        &mut self,
        to_resolve: &Name,
        record_type: RecordType,
        depth: u32,
    ) -> Result<Vec<Record>, ResolutionError> {
        if depth > MAX_RECURSION_DEPTH {
            return Err(ServFail(format!(
                "Refusing to recurse deeper than {}",
                MAX_RECURSION_DEPTH
            )));
        }
        let query_key = (to_resolve.clone(), record_type);
        if self.seen.contains(&query_key) {
            return Err(ServFail(format!("Broken DNS config, seen {:?} twice", query_key)));
        }
        self.seen.push(query_key);

        debug!(hostname = %to_resolve, "Resolving");
        let mut candidates: Box<dyn TargetProvider + Send> =
            Box::new(RootsProvider::new(&self.resolver.roots));
        loop {
            let target = candidates
                .next()
                .await?
                .ok_or_else(|| ServFail("no more nameservers to try".to_string()))?;
            let target = self.target_to_ip(target, depth).await?;
            debug!(%target, "Contacting");
            let response = match self.resolver.backend.query(target, to_resolve, record_type).await
            {
                Err(e) => return Err(e),
                Ok(message) => {
                    if message.response_code() == ResponseCode::NXDomain {
                        return Err(NxDomain);
                    } else if is_final(&message) {
                        Answer(message.answers().to_vec())
                    } else {
                        Referral(message.name_servers().to_vec(), message.additionals().to_vec())
                    }
                }
            };
            match response {
                Referral(ns, glue) => {
                    debug!(?ns, "Received a redirect");
                    candidates = Box::new(NsProvider::new(ns, glue))
                }
                Answer(answers) => return Ok(answers),
            }
        }
    }

    async fn target_to_ip(
        &mut self,
        target: Target,
        depth: u32,
    ) -> Result<IpAddr, ResolutionError> {
        match target {
            Target::Ip(ip) => Ok(ip),
            Target::Name(name) => {
                first_ip(&mut Box::pin(self.resolve_inner(&name, A, depth + 1)).await?)
            }
        }
    }
}

enum QueryResponse {
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
    use std::net::{IpAddr, Ipv4Addr};

    use anyhow::Result;
    use hickory_proto::op::{Header, Message};
    use hickory_proto::rr::{rdata, Record};
    use hickory_proto::rr::{Name, RData, RecordType};
    use tracing::Level;
    use tracing_subscriber::FmtSubscriber;
    use RecordType::A;

    use crate::fake_backend::FakeBackend;
    use crate::resolver::{is_final, RecursiveResolver, ResolutionError};

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

    macro_rules! refer {
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
        let mut b = FakeBackend::new();
        b.add("10.0.0.1", "a.b", A, refer!(ns!("b", "ns.e.f"), a!("ns.e.f", "10.0.0.2")))?;
        b.add("10.0.0.2", "a.b", A, refer!(ns!["a.b", "ns.c.d"]))?;
        b.add("10.0.0.1", "ns.c.d", A, refer!(ns!("c.d", "ns.c.d"), a!("ns.c.d", "10.0.0.3")))?;
        // todo: once using glue records is smarter, remove this
        b.add("10.0.0.3", "ns.c.d", A, answer!(a!("ns.c.d", "10.0.0.3")))?;
        b.add("10.0.0.3", "a.b", A, answer!(a!("a.b", "10.0.0.42")))?;

        let resolver = RecursiveResolver::with_backend(b, vec![IpAddr::V4("10.0.0.1".parse()?)]);

        let result = resolver.resolve(&"a.b".parse()?, A).await?;
        let record = result.first().expect("Could not find record in response");
        assert_eq!(*record.name(), "a.b".parse::<Name>()?);
        if let Some(RData::A(rdata::A(addr))) = record.data() {
            assert_eq!(*addr, "10.0.0.42".parse::<Ipv4Addr>()?)
        } else {
            panic!("Could not find AAAA record in result")
        }

        Ok(())
    }

    #[tokio::test]
    async fn test_cross_referencing_domains() -> Result<()> {
        let mut b = FakeBackend::new();
        b.add("10.0.0.1", "ns.a.b", A, refer!(ns!("b", "ns.e.f"), a!("ns.e.f", "10.0.0.2")))?;

        b.add("10.0.0.2", "ns.a.b", A, refer!(ns!("a.b", "ns.c.d")))?;

        b.add("10.0.0.1", "ns.c.d", A, refer!(ns!("c.d", "e.f.g"), a!("e.f.g", "10.0.0.3")))?;

        // NS record for ns.c.d points back to ns.a.b.
        b.add("10.0.0.3", "ns.c.d", A, refer!(ns!("c.d", "ns.a.b")))?;

        let resolver = RecursiveResolver::with_backend(b, vec![IpAddr::V4("10.0.0.1".parse()?)]);

        let result = resolver.resolve(&"ns.a.b".parse()?, A).await;

        if let Err(ResolutionError::ServFail(e)) = result {
            assert_eq!(format!("{e}"), "Broken DNS config, seen (Name(\"ns.a.b\"), A) twice");
        } else {
            panic!("This resolve() call should fail");
        }

        Ok(())
    }

    #[ctor::ctor]
    fn init() {
        let subscriber = FmtSubscriber::builder().with_max_level(Level::DEBUG).finish();
        tracing::subscriber::set_global_default(subscriber)
            .expect("Could not set global default tracing subscriber");
    }
}

fn first_ip(result: &mut Vec<Record>) -> Result<IpAddr, ResolutionError> {
    match result.pop() {
        None => Err(ServFail("unexpected empty result".to_string())),
        Some(record) => match record.data() {
            Some(RData::A(a)) => Ok(IpAddr::V4(a.0)),
            _ => Err(ServFail("no rdata, or wrong type of rdata".to_string())),
        },
    }
}
