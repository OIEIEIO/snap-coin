use std::net::SocketAddr;

use log::error;
use rand::{rng, seq::IteratorRandom};

use crate::node::{
    message::{Command, Message},
    node::{SharedBlockchain, connect_peer},
    node_state::SharedNodeState,
    peer::{PeerError, PeerHandle},
};

pub const TARGET_PEERS: usize = 12;

async fn get_peer_referrals(peer: &PeerHandle) -> Result<Vec<SocketAddr>, PeerError> {
    if let Command::SendPeers { peers } =
        peer.request(Message::new(Command::GetPeers)).await?.command
    {
        let mut referrals = vec![];
        for peer in peers {
            if let Ok(referral) = peer.parse() {
                referrals.push(referral);
            }
        }
        return Ok(referrals);
    }
    Err(PeerError::Unknown(
        "GetPeers returned incorrect response".into(),
    ))
}

/// Start a Auto Peer daemon, that automatically finds peers to connect to via P2P
pub fn start_auto_peer(node_state: SharedNodeState, blockchain: SharedBlockchain) {
    tokio::spawn(async move {
        loop {
            // Get a random peer, that we will poll for peers.
            let selected_peer = {
                let peers = node_state.connected_peers.read().await;

                let mut rng = rng();
                peers
                    .iter()
                    .filter(|peer| !peer.1.is_client)
                    .choose(&mut rng)
                    .map(|(_, peer)| peer.clone())
            };

            let blockchain = &blockchain;
            let node_state = &node_state;
            if let Some(peer) = selected_peer {
                let peer_address = peer.address;
                if let Err(e) = async move {
                    let referrals = get_peer_referrals(&peer).await?;

                    for referral in referrals {
                        connect_peer(referral, blockchain, node_state).await?;
                    }

                    Ok::<(), PeerError>(())
                }
                .await
                {
                    error!("Auto peer failed {peer_address}, error: {e}");
                }
            }
        }
    });
}
