use std::fmt::Debug;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};

use crate::resolver::ResolutionError;
use async_trait::async_trait;
use hickory_proto::op::{Message, Query};
use hickory_proto::rr::Name;
use hickory_proto::rr::RecordType;
use hickory_proto::serialize::binary::BinDecodable;
use tokio::net::UdpSocket;
use tracing::field::Empty;
use tracing::instrument;

/// Max size for the UDP receive buffer as recommended by
/// [RFC6891](https://datatracker.ietf.org/doc/html/rfc6891#section-6.2.5).
pub const MAX_RECEIVE_BUFFER_SIZE: usize = 4096;

const DEFAULT_TARGET_PORT: u16 = 53;

/// A backend represents something that can pass on queries and potentially return responses
/// from the remote that the query was sent to.
#[async_trait]
pub trait Backend: Debug {
    async fn query(
        &self,
        target: IpAddr,
        to_resolve: &Name,
        record_type: RecordType,
    ) -> Result<Message, ResolutionError>;
}

/// A Backend implementation that implements the DNS query request/response
/// behaviour with UDP messages to a remote host.
#[derive(Debug)]
pub struct UdpBackend {
    target_port: u16,
}

impl UdpBackend {
    pub fn new() -> Self {
        UdpBackend { target_port: DEFAULT_TARGET_PORT }
    }
}

async fn connect(target: IpAddr, target_port: u16) -> Result<UdpSocket, ResolutionError> {
    let local = SocketAddr::new(
        match target {
            IpAddr::V4(_) => IpAddr::V4(Ipv4Addr::UNSPECIFIED),
            IpAddr::V6(_) => IpAddr::V6(Ipv6Addr::UNSPECIFIED),
        },
        0,
    );
    let socket = UdpSocket::bind(local).await?;
    socket.connect(SocketAddr::new(target, target_port)).await?;
    Ok(socket)
}

#[async_trait]
impl Backend for UdpBackend {
    // It looks a little weird to have status be set to error, but this is being overwritten
    // unless the ? operator makes the execution return early
    #[instrument(fields(otel.status_code = "Error", result = Empty, %to_resolve, %record_type, response_code = Empty))]
    async fn query(
        &self,
        target: IpAddr,
        to_resolve: &Name,
        record_type: RecordType,
    ) -> Result<Message, ResolutionError> {
        let socket = connect(target, self.target_port).await?;

        let request = make_query(to_resolve, record_type);
        socket.send(request.to_vec()?.as_slice()).await?;
        let mut buf = vec![0u8; MAX_RECEIVE_BUFFER_SIZE];
        let read_count = socket.recv(&mut buf).await?;

        let message = Message::from_bytes(&buf[..read_count])?;
        let span = tracing::Span::current();
        span.record("otel.status_code", "Ok");
        span.record("result", format!("{:?}", message));
        span.record("response_code", format!("{}", message.header().response_code()));
        Ok(message)
    }
}

fn make_query(name: &Name, record_type: RecordType) -> Message {
    let mut query = Query::new();
    query.set_name(name.clone()).set_query_type(record_type);
    let mut message = Message::new();
    message.add_query(query);
    message.set_recursion_desired(true);
    message.set_id(rand::random());
    message.set_authentic_data(true);
    message
}

#[cfg(test)]
mod test {
    use hickory_proto::op::{Message, ResponseCode};
    use hickory_proto::rr::rdata::A;
    use hickory_proto::rr::{Name, RData, Record, RecordType};
    use hickory_proto::serialize::binary::BinDecodable;
    use std::net::{IpAddr, Ipv4Addr, SocketAddr};
    use std::str::FromStr;
    use tokio::net::UdpSocket;
    use tokio::task::JoinHandle;

    use crate::backend::Backend;
    use crate::backend::{UdpBackend, MAX_RECEIVE_BUFFER_SIZE};
    use crate::resolver::ResolutionError;
    use anyhow::Result;

    async fn verify_request_send_response(
    ) -> Result<(u16, JoinHandle<Result<(), ResolutionError>>), ResolutionError> {
        let server_socket =
            UdpSocket::bind(SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0)).await?;
        let port = server_socket.local_addr()?.port();
        let handler = tokio::spawn(async move {
            let mut buf = vec![0u8; MAX_RECEIVE_BUFFER_SIZE];
            let (read_count, peer) = server_socket.recv_from(&mut buf).await?;
            let req = Message::from_bytes(&buf[..read_count])?;
            let resp = make_response(req);
            server_socket.send_to(resp.to_vec()?.as_slice(), peer).await?;
            Ok(())
        });
        Ok((port, handler))
    }

    fn make_response(request: Message) -> Message {
        let mut message = Message::new();
        message.add_query(request.query().unwrap().clone());
        message.set_id(request.id());
        message.set_response_code(ResponseCode::NoError);
        message.add_answer(Record::from_rdata(
            Name::from_str("stacey.a.b.").unwrap(),
            600,
            RData::A(A::new(172, 104, 148, 31)),
        ));
        message
    }

    #[tokio::test]
    async fn test_udp_interaction() -> Result<()> {
        let (port, handle) = verify_request_send_response().await?;

        let b = UdpBackend { target_port: port };
        let message =
            b.query(IpAddr::V4(Ipv4Addr::LOCALHOST), &"stacey.a.b".parse()?, RecordType::A).await?;
        assert_eq!(message.response_code(), ResponseCode::NoError);
        let answers = message.answers();
        let expected = Record::from_rdata(
            Name::from_str("stacey.a.b.")?,
            600,
            RData::A("172.104.148.31".parse()?),
        );
        assert_eq!(answers, [expected]);
        handle.await??;
        Ok(())
    }
}
