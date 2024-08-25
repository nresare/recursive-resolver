use std::collections::HashMap;
use std::fmt::{Debug, Formatter};
use std::net::IpAddr;

use async_trait::async_trait;
use hickory_proto::op::Message;
use hickory_proto::rr::{Name, RecordType};

use crate::backend::Backend;
use crate::resolver::ResolutionError;
use crate::resolver::ResolutionError::ServFail;

pub struct FakeBackend {
    answers: HashMap<QueryKey, Message>,
}

pub struct ServFailBackend {}

#[async_trait]
impl Backend for ServFailBackend {
    async fn query(&self, _: IpAddr, _: &Name, _: RecordType) -> Result<Message, ResolutionError> {
        Err(ServFail("from ServFailBackend".to_string()))
    }
}
impl Debug for ServFailBackend {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ServFailBackend").finish()
    }
}

impl Debug for FakeBackend {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FakeBackend").field("answer_count", &self.answers.len()).finish()
    }
}

impl FakeBackend {
    pub fn new() -> Self {
        FakeBackend { answers: HashMap::new() }
    }
    pub fn add(
        &mut self,
        ip: &str,
        name: &str,
        record_type: RecordType,
        message: Message,
    ) -> Result<(), ResolutionError> {
        let key = QueryKey {
            target: IpAddr::V4(ip.parse().expect("Failed to parse IP")),
            name: name.parse()?,
            record_type,
        };
        self.answers.insert(key, message);
        Ok(())
    }

    pub fn get(&self, target: IpAddr, name: &Name, record_type: RecordType) -> Option<Message> {
        let key = QueryKey { target, name: name.clone(), record_type };

        self.answers.get(&key).cloned()
    }
}
#[derive(PartialEq, Eq, Hash)]
pub struct QueryKey {
    target: IpAddr,
    name: Name,
    record_type: RecordType,
}

#[async_trait]
impl Backend for FakeBackend {
    async fn query(
        &self,
        target: IpAddr,
        name: &Name,
        record_type: RecordType,
    ) -> Result<Message, ResolutionError> {
        self.get(target, name, record_type).ok_or(ServFail(format!(
            "Could not find response for {name} {record_type} at {target}"
        )))
    }
}
