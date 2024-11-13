use crate::backend::MAX_RECEIVE_BUFFER_SIZE;
use crate::resolver::{RecursiveResolver, ResolutionError};
use hickory_proto::op::{Message, ResponseCode};
use hickory_proto::serialize::binary::BinDecodable;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::ops::Deref;
use std::sync::Arc;
use tokio::net::UdpSocket;

pub async fn daemon(resolver: RecursiveResolver, listen_port: u16) -> anyhow::Result<()> {
    let sock =
        UdpSocket::bind(SocketAddr::new(IpAddr::V4(Ipv4Addr::UNSPECIFIED), listen_port)).await?;
    let r = Arc::new(sock);
    let resolver = Arc::new(resolver);

    let mut buf = [0; MAX_RECEIVE_BUFFER_SIZE];
    loop {
        let (msg, peer) = read_message(r.deref(), &mut buf).await?;
        tokio::spawn(handle(r.clone(), msg, peer, resolver.clone()));
    }
}

async fn handle(
    socket: Arc<UdpSocket>,
    msg: Message,
    peer: SocketAddr,
    resolver: Arc<RecursiveResolver>,
) -> anyhow::Result<()> {
    let response = resolve(msg, &resolver).await;
    socket.send_to(response.to_vec()?.as_slice(), peer).await?;
    Ok(())
}

async fn resolve(message: Message, resolver: &RecursiveResolver) -> Message {
    let mut response = Message::new();
    response.set_id(message.id());
    let Some(query) = message.query() else {
        response.set_response_code(ResponseCode::FormErr);
        return response;
    };

    match resolver.resolve(query.name(), query.query_type()).await {
        Ok(records) => {
            for r in records {
                response.add_answer(r);
            }
        }
        Err(ResolutionError::NxDomain) => {
            response.set_response_code(ResponseCode::NXDomain);
        }
        Err(_) => {
            response.set_response_code(ResponseCode::ServFail);
        }
    }
    response
}

async fn read_message(socket: &UdpSocket, buf: &mut [u8]) -> anyhow::Result<(Message, SocketAddr)> {
    let (bytes_read, addr) = socket.recv_from(buf).await?;
    Ok((Message::from_bytes(&buf[..bytes_read])?, addr))
}

#[cfg(test)]
mod test {
    use crate::daemon::resolve;
    use crate::fake_backend::ServFailBackend;
    use crate::resolver::RecursiveResolver;
    use hickory_proto::op::{Message, Query, ResponseCode};

    #[tokio::test]
    async fn test_resolve_non_query() {
        // no query set, should return a servfail
        let mut msg = Message::new();
        msg.set_id(4711);
        let response = resolve(msg, &RecursiveResolver::new()).await;
        assert_eq!(response.header().response_code(), ResponseCode::FormErr);
        assert_eq!(4711, response.id());
    }

    #[tokio::test]
    async fn test_resolve_servfail() {
        let resolver = RecursiveResolver::with_backend(ServFailBackend {}, vec![]);
        let mut msg = Message::new();
        msg.set_id(4712);
        msg.add_query(Query::new());
        let response = resolve(msg, &resolver).await;
        assert_eq!(response.header().response_code(), ResponseCode::ServFail);
        assert_eq!(4712, response.id());
    }
}
