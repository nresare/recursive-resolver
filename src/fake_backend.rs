use crate::backend::Backend;
use anyhow::{anyhow, Result};
use async_trait::async_trait;
use hickory_proto::op::Message;
use hickory_proto::rr::{Name, RecordType};
use std::collections::HashMap;
use std::net::IpAddr;

pub struct FakeBackend {
    answers: HashMap<QueryKey, Message>,
}

impl FakeBackend {
    pub fn new() -> Self {
        FakeBackend {
            answers: HashMap::new(),
        }
    }
    pub fn add(
        &mut self,
        ip: &str,
        name: &str,
        record_type: RecordType,
        message: Message,
    ) -> Result<()> {
        let key = QueryKey {
            target: IpAddr::V4(ip.parse()?),
            name: name.parse()?,
            record_type,
        };
        self.answers.insert(key, message);
        Ok(())
    }

    pub fn get(&self, target: IpAddr, name: &Name, record_type: RecordType) -> Option<Message> {
        let key = QueryKey {
            target,
            name: name.clone(),
            record_type,
        };

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
    async fn query(&self, target: IpAddr, name: &Name, record_type: RecordType) -> Result<Message> {
        self.get(target, name, record_type).ok_or(anyhow!(
            "Could not find response for {name} {record_type} at {target}"
        ))
    }
}
