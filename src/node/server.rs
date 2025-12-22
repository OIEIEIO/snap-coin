use std::net::{IpAddr, Ipv4Addr, SocketAddr};

use log::error;
use thiserror::Error;
use tokio::{net::TcpListener, task::JoinHandle};

use crate::node::{node::SharedBlockchain, node_state::SharedNodeState, peer::create_peer};

#[derive(Error, Debug)]
pub enum ServerError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
}

pub async fn start_server(
    port: u16,
    blockchain: SharedBlockchain,
    node_state: SharedNodeState,
) -> Result<JoinHandle<()>, ServerError> {
    let listener =
        TcpListener::bind(SocketAddr::new(IpAddr::V4(Ipv4Addr::UNSPECIFIED), port)).await?;

    Ok(tokio::spawn(async move {
        while let Ok((stream, address)) = listener.accept().await {
            match create_peer(stream, blockchain.clone(), node_state.clone(), true) {
                Ok(handle) => {
                    node_state
                        .connected_peers
                        .write()
                        .await
                        .insert(address, handle);
                }
                Err(e) => {
                    error!("Failed to connect to create (incoming) peer : {e}");
                }
            }
        }
    }))
}
