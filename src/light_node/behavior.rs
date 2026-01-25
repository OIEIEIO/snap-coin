use std::sync::Arc;

use log::warn;

use crate::{
    light_node::{SharedLightNodeState, accept_block, accept_transaction},
    node::{
        message::{Command, Message},
        peer::{PeerError, PeerHandle},
        peer_behavior::{PeerBehavior, SharedPeerBehavior},
    },
};

pub struct LightNodePeerBehavior {
    light_node_state: SharedLightNodeState,
}

impl LightNodePeerBehavior {
    pub fn new(light_node_state: SharedLightNodeState) -> SharedPeerBehavior {
        Arc::new(Self { light_node_state })
    }
}

#[async_trait::async_trait]
impl PeerBehavior for LightNodePeerBehavior {
    async fn on_message(&self, message: Message, _peer: &PeerHandle) -> Result<Message, PeerError> {
        let light_node_state = &self.light_node_state;
        let response = match message.command {
            Command::Connect => message.make_response(Command::AcknowledgeConnection),
            Command::AcknowledgeConnection => {
                return Err(PeerError::Unknown(
                    "Got unhandled AcknowledgeConnection".to_string(),
                ));
            }
            Command::Ping { height } => message.make_response(Command::Pong { height }),
            Command::Pong { .. } => {
                return Err(PeerError::Unknown("Got unhandled Ping".to_string()));
            }
            Command::GetPeers => message.make_response(Command::SendPeers { peers: vec![] }),
            Command::SendPeers { .. } => {
                return Err(PeerError::Unknown("Got unhandled SendPeers".to_string()));
            }
            Command::NewBlock { ref block } => {
                // if !*light_node_state.is_syncing.read().await {
                match accept_block(&light_node_state, block.clone()).await {
                    Ok(()) => {}
                    Err(e) => {
                        warn!("Incoming block is invalid: {e}")
                    }
                }
                // }

                message.make_response(Command::NewBlockResolved)
            }
            Command::NewBlockResolved => {
                return Err(PeerError::Unknown(
                    "Got unhandled NewBlockAccepted".to_string(),
                ));
            }
            Command::NewTransaction { ref transaction } => {
                match accept_transaction(&light_node_state, transaction.clone()).await {
                    Ok(()) => {}
                    Err(e) => {
                        warn!("Incoming transaction is invalid: {e}")
                    }
                }
                message.make_response(Command::NewTransactionResolved)
            }
            Command::NewTransactionResolved => {
                return Err(PeerError::Unknown(
                    "Got unhandled NewTransactionAccepted".to_string(),
                ));
            }
            Command::GetBlock { .. } => message.make_response(Command::GetBlockResponse {
                block: None, // We do not store blocks
            }),
            Command::GetBlockResponse { .. } => {
                return Err(PeerError::Unknown(
                    "Got unhandled GetBlockResponse".to_string(),
                ));
            }
            Command::GetBlockHashes { start, end } => {
                let mut hashes = vec![];
                for height in start..end {
                    if let Some(meta) = light_node_state.meta_store().get_meta_by_height(height)
                        && let Some(hash) = meta.hash
                    {
                        hashes.push(hash);
                    }
                }
                message.make_response(Command::GetBlockHashesResponse {
                    block_hashes: hashes,
                })
            }
            Command::GetBlockHashesResponse { .. } => {
                return Err(PeerError::Unknown(
                    "Got unhandled GetBlockResponse".to_string(),
                ));
            }
            Command::GetTransactionMerkleProof { .. } => {
                message.make_response(Command::GetTransactionMerkleProofResponse { proof: None })
            } // We don't give merkle proofs as we don't have all blocks
            Command::GetTransactionMerkleProofResponse { .. } => {
                return Err(PeerError::Unknown(
                    "Got unhandled GetTransactionMerkleProofResponse".to_string(),
                ));
            }
            Command::GetBlockMetadata { block_hash } => {
                let block_metadata = light_node_state.meta_store().get_meta_by_hash(block_hash);
                message.make_response(Command::GetBlockMetadataResponse { block_metadata })
            }
            Command::GetBlockMetadataResponse { .. } => {
                return Err(PeerError::Unknown(
                    "Got unhandled GetBlockMetadataResponse".to_string(),
                ));
            }
        };

        Ok(response)
    }

    async fn get_height(&self) -> usize {
        self.light_node_state.meta_store().get_height()
    }

    async fn on_kill(&self, peer: &PeerHandle) {
        self.light_node_state
            .connected_peers
            .write()
            .await
            .remove(&peer.address);
    }
}
