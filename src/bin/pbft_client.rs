use pbft::messages::{Message, ClientRequest};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt ,BufReader,  BufStream};
use tokio::{net::TcpListener, net::TcpStream};
use std::net::{SocketAddr, IpAddr, Ipv4Addr};

use serde_json;

#[tokio::main]
async fn main() -> std::io::Result<()> {
    let me_addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)), 8079);
    let listener = TcpListener::bind(me_addr.clone()).await.unwrap();
    let mut node_addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)), 8061);

    let mut node_stream = TcpStream::connect(node_addr).await.unwrap();
    let message : Message = Message::ClientRequestMessage(ClientRequest {
        respond_addr: me_addr,
        time_stamp: 0,
        key: String::from("abc"),
        value: 3, 
    });
    let _bytes_written = node_stream
            .write(message.serialize().as_slice())
            .await?;

    loop {
        match listener.accept().await {
            Ok((stream, _)) => {
                tokio::spawn(async move {
                    handle_stream(stream).await;
                });
            }
            Err(e) => {
                return Err(e);
            }
        }
    }
}

async fn handle_stream(mut stream: TcpStream) -> std::io::Result<()> {
    println!("Handling stream {:?}", stream.peer_addr());
    let mut reader = BufReader::new(&mut stream);
    loop {
        let mut res = String::new();
        let bytes_read = reader.read_line(&mut res).await?;
        if bytes_read == 0 {
            println!("Breaking connection;");
            return Ok(());
        }
        let message: Message = serde_json::from_str(&res).unwrap();
        println!("Got: {:?}", message);
    }
}
