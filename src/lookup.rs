use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};

use anyhow::Result;
use hickory_resolver::Name;
use hickory_resolver::proto::op::{Message, Query};
use hickory_resolver::proto::rr::RecordType;
use hickory_resolver::proto::serialize::binary::BinDecodable;
use tokio::net::UdpSocket;

/// Max size for the UDP receive buffer as recommended by
/// [RFC6891](https://datatracker.ietf.org/doc/html/rfc6891#section-6.2.5).
const MAX_RECEIVE_BUFFER_SIZE: usize = 4096;

const DEFAULT_TARGET_PORT: u16 = 53;

/// A backend represents something that can pass on queries and potentially return responses
/// from the remote that the query was sent to.
trait Backend {
    async fn query(&self, target: IpAddr, name: Name, record_type: RecordType) -> Result<Message>;
}

struct UdpBackend {
    target_port: u16,
}

impl UdpBackend {
    pub fn new() -> Self {
        UdpBackend { target_port: DEFAULT_TARGET_PORT }
    }
}

async fn connect(target: IpAddr, target_port: u16) -> Result<UdpSocket> {
    let local = SocketAddr::new(match target {
        IpAddr::V4(_) => IpAddr::V4(Ipv4Addr::UNSPECIFIED),
        IpAddr::V6(_) => IpAddr::V6(Ipv6Addr::UNSPECIFIED),
    }, 0);
    let socket = UdpSocket::bind(local).await?;
    socket.connect(SocketAddr::new(target, target_port)).await?;
    Ok(socket)
}

impl Backend for UdpBackend {
    async fn query(&self, target: IpAddr, name: Name, record_type: RecordType) -> Result<Message> {
        let socket = connect(target, self.target_port).await?;

        let request = make_query(name, record_type);
        println!("Req: {:?}", request);
        socket.send(request.to_vec()?.as_slice()).await?;
        let mut buf = vec![0u8; MAX_RECEIVE_BUFFER_SIZE];
        let read_count = socket.recv(&mut buf).await?;

        Ok(Message::from_bytes(&buf[..read_count])?)
    }
}

fn make_query(name: Name, record_type: RecordType) -> Message {
    let mut query = Query::new();
    query.set_name(name).set_query_type(record_type);
    let mut message = Message::new();
    message.add_query(query);
    message.set_recursion_desired(true);
    message.set_id(rand::random());
    message.set_authentic_data(true);
    message
}

#[cfg(test)]
mod test {
    use std::net::{IpAddr, Ipv4Addr, SocketAddr};

    use anyhow::Result;
    use hickory_resolver::proto::rr::RecordType;
    use tokio::net::UdpSocket;
    use tokio::task::JoinHandle;

    use crate::lookup::{MAX_RECEIVE_BUFFER_SIZE, UdpBackend};
    use crate::lookup::Backend;

    async fn verify_request_send_response() -> Result<(u16, JoinHandle<Result<()>>)> {
        let server_socket = UdpSocket::bind(SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0)).await?;
        let port = server_socket.local_addr()?.port();
        let handler = tokio::spawn(async move {
            let mut buf = vec![0u8; MAX_RECEIVE_BUFFER_SIZE];
            let (read_count, peer) = server_socket.recv_from(&mut buf).await?;
            server_socket.send_to(&buf[..read_count], peer).await?;
            Ok(())
        });
        Ok((port, handler))
    }

    #[tokio::test]
    async fn test_udp_interaction() -> Result<()> {
        let (port, handle) = verify_request_send_response().await?;

        let b = UdpBackend { target_port: port };
        let message = b.query(
            IpAddr::V4(Ipv4Addr::LOCALHOST),
            "stacey.noa.re".parse()?,
            RecordType::A
        ).await?;
        print!("{:?}", message);

        handle.await??;
        Ok(())
    }
}