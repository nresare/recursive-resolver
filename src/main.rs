use std::net::SocketAddr;

use anyhow::Result;
use futures_util::stream::StreamExt;
use futures_util::FutureExt;
use hickory_resolver::config::ResolverOpts;
use hickory_resolver::config::{NameServerConfig, Protocol};
use hickory_resolver::error::ResolveError;
use hickory_resolver::name_server::{GenericConnector, NameServer, TokioRuntimeProvider};
use hickory_resolver::proto::op::{Message, MessageType, OpCode, Query};
use hickory_resolver::proto::rr::RecordType;
use hickory_resolver::proto::xfer::{DnsRequest, DnsRequestOptions, DnsResponse};
use hickory_resolver::proto::DnsHandle;
use hickory_resolver::Name;

mod backend;
mod resolver;

#[tokio::main]
async fn main() -> Result<()> {
    let ns = build_nameserver()?;
    let query = Query::query(Name::parse("stacey.resare.com", None)?, RecordType::AAAA);
    let result = lookup(query, ns).await?;
    println!("{:?}", result);
    Ok(())
}

fn build_nameserver() -> Result<NameServer<GenericConnector<TokioRuntimeProvider>>> {
    let socket_addr: SocketAddr = "192.168.168.1:53".parse()?;
    let config = NameServerConfig {
        socket_addr,
        protocol: Protocol::Udp,
        tls_dns_name: None,
        trust_negative_responses: false,
        bind_addr: None,
    };
    Ok(NameServer::new(
        config,
        ResolverOpts::default(),
        GenericConnector::new(TokioRuntimeProvider::default()),
    ))
}

async fn lookup(
    query: Query,
    ns: NameServer<GenericConnector<TokioRuntimeProvider>>,
) -> Result<DnsResponse, ResolveError> {
    let options = DnsRequestOptions::default();
    let request: DnsRequest = DnsRequest::new(build_message(query), options);
    let future = ns.send(request).into_future();

    future
        .map(|(next, _)| next.map(|r| r.map_err(ResolveError::from)))
        .await
        .expect("Can't deal with empty results for now")
}

fn build_message(query: Query) -> Message {
    // build the message
    let mut message: Message = Message::new();
    // TODO: This is not the final ID, it's actually set in the poll method of DNS future
    //  should we just remove this?
    let id: u16 = rand::random();
    message
        .add_query(query)
        .set_id(id)
        .set_message_type(MessageType::Query)
        .set_op_code(OpCode::Query)
        .set_recursion_desired(true);
    message
}
