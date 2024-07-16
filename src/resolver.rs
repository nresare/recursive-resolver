use std::net::IpAddr;

use anyhow::Result;
use hickory_resolver::proto::op::{Message, ResponseCode};
use hickory_resolver::proto::rr::{Record, RecordType};
use hickory_resolver::Name;
use tracing::debug;

use crate::backend::{Backend, UdpBackend};
use crate::resolver::QueryResponse::{Answer, Referral};
use crate::selector::{IpProvider, NsProvider, RootsProvider};

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

    fn from_backend(backend: impl Backend + Send + Sync + 'static) -> Self {
        RecursiveResolver {
            backend: Box::new(backend),
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
    use std::collections::HashMap;
    use std::net::IpAddr;

    use anyhow::Result;
    use async_trait::async_trait;
    use hickory_proto::rr::{Name, RecordType};
    use hickory_resolver::proto::op::{Header, Message};
    use hickory_resolver::proto::rr::Record;

    use crate::backend::Backend;
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
        let resolver = RecursiveResolver::from_backend(FakeBackend::new());
        let result = resolver.resolve(&"noa.tm".parse()?, RecordType::AAAA).await;
        assert!(matches!(result, Err(_)));
        Ok(())
    }

    struct FakeBackend {
        answers: HashMap<QueryKey, Message>,
    }

    impl FakeBackend {
        fn new() -> Self {
            FakeBackend {
                answers: HashMap::new(),
            }
        }
        fn add(
            &mut self,
            ip: &str,
            name: &str,
            record_type: RecordType,
            answers: Vec<Record>,
            glue: Vec<Record>,
        ) -> Result<()> {
            let key = QueryKey {
                target: IpAddr::V4(ip.parse()?),
                name: name.parse()?,
                record_type,
            };
            let mut message = Message::new();
            message.insert_answers(answers);
            message.insert_additionals(glue);
            self.answers.insert(key, message);
            Ok(())
        }

        fn get(&self, target: IpAddr, name: Name, record_type: RecordType) -> Option<Message> {
            let key = QueryKey {
                target,
                name,
                record_type,
            };

            self.answers.get(&key).map(|v| v.clone())
        }
    }

    #[derive(PartialEq, Eq, Hash)]
    struct QueryKey {
        target: IpAddr,
        name: Name,
        record_type: RecordType,
    }

    impl QueryKey {}

    #[async_trait]
    impl Backend for FakeBackend {
        async fn query(
            &self,
            target: IpAddr,
            name: &Name,
            record_type: RecordType,
        ) -> Result<Message> {
            Err(anyhow::Error::msg("intentionally failing"))
        }
    }
}
