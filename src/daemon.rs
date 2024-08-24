use crate::backend::MAX_RECEIVE_BUFFER_SIZE;
use crate::resolver::RecursiveResolver;
use hickory_proto::op::Message;
use hickory_proto::serialize::binary::BinDecodable;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::ops::Deref;
use std::sync::Arc;
use tokio::net::UdpSocket;
use tracing::debug;

pub async fn daemon(resolver: RecursiveResolver, listen_port: u16) -> anyhow::Result<()> {
    let sock =
        UdpSocket::bind(SocketAddr::new(IpAddr::V4(Ipv4Addr::UNSPECIFIED), listen_port)).await?;
    let r = Arc::new(sock);
    let resolver = Arc::new(resolver);

    let mut buf = [0; MAX_RECEIVE_BUFFER_SIZE];
    loop {
        let (msg, peer) = read_message(r.deref(), &mut buf).await?;
        debug!("read message {msg}");
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
    let query = message.query().expect("Expected query here");
    let records = resolver.resolve(query.name(), query.query_type()).await.unwrap();
    let mut response = Message::new();
    for r in records {
        response.add_answer(r);
    }
    response.set_id(message.id());
    response
}

async fn read_message(socket: &UdpSocket, buf: &mut [u8]) -> anyhow::Result<(Message, SocketAddr)> {
    let (bytes_read, addr) = socket.recv_from(buf).await?;
    Ok((Message::from_bytes(&buf[..bytes_read])?, addr))
}
