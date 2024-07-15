use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};

use anyhow::Result;
use async_trait::async_trait;
use hickory_resolver::proto::op::{Message, Query};
use hickory_resolver::proto::rr::RecordType;
use hickory_resolver::proto::serialize::binary::BinDecodable;
use hickory_resolver::Name;
use tokio::net::UdpSocket;

/// Max size for the UDP receive buffer as recommended by
/// [RFC6891](https://datatracker.ietf.org/doc/html/rfc6891#section-6.2.5).
const MAX_RECEIVE_BUFFER_SIZE: usize = 4096;

const DEFAULT_TARGET_PORT: u16 = 53;

/// A backend represents something that can pass on queries and potentially return responses
/// from the remote that the query was sent to.
#[async_trait]
pub trait Backend {
    async fn query(&self, target: IpAddr, name: &Name, record_type: RecordType) -> Result<Message>;
}

pub struct UdpBackend {
    target_port: u16,
}

impl UdpBackend {
    pub fn new() -> Self {
        UdpBackend {
            target_port: DEFAULT_TARGET_PORT,
        }
    }
}

async fn connect(target: IpAddr, target_port: u16) -> Result<UdpSocket> {
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
    async fn query(&self, target: IpAddr, name: &Name, record_type: RecordType) -> Result<Message> {
        let socket = connect(target, self.target_port).await?;

        let request = make_query(name, record_type);
        socket.send(request.to_vec()?.as_slice()).await?;
        let mut buf = vec![0u8; MAX_RECEIVE_BUFFER_SIZE];
        let read_count = socket.recv(&mut buf).await?;

        Ok(Message::from_bytes(&buf[..read_count])?)
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
    use anyhow::Result;
    use hickory_resolver::proto::op::{Message, ResponseCode};
    use hickory_resolver::proto::rr::rdata::A;
    use hickory_resolver::proto::rr::{RData, Record, RecordType};
    use hickory_resolver::proto::serialize::binary::BinDecodable;
    use hickory_resolver::Name;
    use std::net::{IpAddr, Ipv4Addr, SocketAddr};
    use std::str::FromStr;
    use tokio::net::UdpSocket;
    use tokio::task::JoinHandle;

    use crate::backend::Backend;
    use crate::backend::{UdpBackend, MAX_RECEIVE_BUFFER_SIZE};

    async fn verify_request_send_response() -> Result<(u16, JoinHandle<Result<()>>)> {
        let server_socket =
            UdpSocket::bind(SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0)).await?;
        let port = server_socket.local_addr()?.port();
        let handler = tokio::spawn(async move {
            let mut buf = vec![0u8; MAX_RECEIVE_BUFFER_SIZE];
            let (read_count, peer) = server_socket.recv_from(&mut buf).await?;
            let req = Message::from_bytes(&buf[..read_count])?;
            let resp = make_response(req);
            server_socket
                .send_to(&resp.to_vec()?.as_slice(), peer)
                .await?;
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
            Name::from_str("stacey.noa.re.").unwrap(),
            600,
            RData::A(A::new(172, 104, 148, 31)),
        ));
        message
    }

    #[tokio::test]
    async fn test_udp_interaction() -> Result<()> {
        let (port, handle) = verify_request_send_response().await?;

        let b = UdpBackend { target_port: port };
        let message = b
            .query(
                IpAddr::V4(Ipv4Addr::LOCALHOST),
                &"stacey.noa.re".parse()?,
                RecordType::A,
            )
            .await?;
        assert_eq!(message.response_code(), ResponseCode::NoError);
        let answers = message.answers();
        let expected = Record::from_rdata(
            Name::from_str("stacey.noa.re.")?,
            600,
            RData::A("172.104.148.31".parse()?),
        );
        assert_eq!(answers, [expected]);
        handle.await??;
        Ok(())
    }
}
