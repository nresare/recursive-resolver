use std::net::IpAddr;

use crate::backend::{Backend, UdpBackend};
use crate::resolver::QueryResponse::{Answer, Referral};
use crate::selector::{IpProvider, NsProvider, RootsProvider};
use anyhow::Result;
use hickory_resolver::proto::op::{Message, ResponseCode};
use hickory_resolver::proto::rr::{Record, RecordType};
use hickory_resolver::Name;
use tracing::debug;

pub(crate) struct RecursiveResolver {
    backend: Box<dyn Backend + Sync + Send>,
    roots: Vec<IpAddr>,
}

impl RecursiveResolver {
    pub(crate) fn new() -> Self {
        RecursiveResolver {
            backend: Box::new(UdpBackend::new()),
            roots: vec![
                IpAddr::V4("192.36.148.17".parse().unwrap()),
                //IpAddr::V6("2001:7fe::53".parse().unwrap()),
            ],
        }
    }

    //#[instrument]
    pub(crate) async fn resolve(
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
                    debug!("Received a redirect");
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
    use anyhow::Result;
    use hickory_proto::rr::RecordType;
    use hickory_resolver::proto::op::{Header, Message};
    use hickory_resolver::proto::rr::Record;

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

    #[tokio::test]
    async fn test_resolve() -> Result<()> {
        let resolver = RecursiveResolver::new();
        let result = resolver
            .resolve(&"noa.tm".parse()?, RecordType::AAAA)
            .await?;
        println!("result: {:?}", result);
        Ok(())
    }
}
