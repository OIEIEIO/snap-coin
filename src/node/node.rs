use flexi_logger::{Duplicate, FileSpec, Logger};
use futures::future::join_all;
use log::{error, info};
use num_bigint::BigUint;
use tokio::net::TcpStream;
use std::{
    net::SocketAddr, path::PathBuf, sync::{Arc, Once}
};

use crate::{
    core::{
        block::Block,
        blockchain::{self, Blockchain, BlockchainError},
        transaction::Transaction,
    },
    node::{
        message::{Command, Message},
        node_state::{NodeState, SharedNodeState},
        peer::{self, PeerError, PeerHandle, create_peer, kill_peer},
    },
};

pub type SharedBlockchain = Arc<Blockchain>;

static LOGGER_INIT: Once = Once::new();

pub struct Node {
    pub node_state: SharedNodeState,
    pub blockchain: SharedBlockchain,
}

impl Node {
    pub fn new(node_path: &str) -> Node {
        let node_path = PathBuf::from(node_path);

        // Only initialize the logger once
        LOGGER_INIT.call_once(|| {
            let log_path = node_path.join("logs");
            std::fs::create_dir_all(&log_path).expect("Failed to create log directory");

            Logger::try_with_str("info")
                .unwrap()
                .log_to_file(FileSpec::default().directory(&log_path))
                .duplicate_to_stderr(Duplicate::Info)
                .start()
                .ok(); // Ignore errors if logger is already set

            info!("Logger initialized for node at {:?}", node_path);
        });

        let node_state = NodeState::new_empty();
        node_state.mempool.start_expiry_watchdog();

        Node {
            blockchain: Arc::new(Blockchain::new(
                node_path
                    .join("blockchain")
                    .to_str()
                    .expect("Failed to create node path"),
            )),
            node_state,
        }
    }

    pub async fn connect_peer(&self, address: SocketAddr) -> Result<PeerHandle, PeerError> {
        let stream = TcpStream::connect(address).await.map_err(|e| PeerError::Io(format!("IO error: {e}")))?;

        let handle = create_peer(stream, self.blockchain.clone(), self.node_state.clone(), false)?;
        self.node_state.connected_peers.write().await.insert(address, handle.clone());

        Ok(handle)
    }
}

/// Forward a message to all peers
pub async fn to_peers(message: Message, node_state: &SharedNodeState) {
    let peers_snapshot: Vec<_> = node_state
        .connected_peers
        .read()
        .await
        .values()
        .cloned()
        .collect();

    // Create a list of futures for all peers
    let futures = peers_snapshot.into_iter().map(|peer| {
        let message = message.clone();
        async move {
            if let Err(err) = peer::request_from_peer(&peer, message).await {
                if let Err(e) = kill_peer(&peer, err.to_string()).await {
                    error!("Failed to kill peer, error: {e}");
                }
            }
        }
    });

    // Run all futures concurrently
    join_all(futures).await;
}

/// Accept a new block to the local blockchain, and forward it to all peers
pub async fn accept_block(
    blockchain: &SharedBlockchain,
    node_state: &SharedNodeState,
    new_block: Block,
) -> Result<(), BlockchainError> {
    new_block.check_completeness()?;
    let block_hash = new_block.meta.hash.unwrap(); // Unwrap is okay, we checked that block is complete

    if node_state.last_seen_block() == block_hash {
        return Ok(()); // We already processed this block
    }
    node_state.set_last_seen_block(block_hash);

    // Validation
    blockchain::validate_block_timestamp(&new_block)?;
    blockchain.add_block(new_block.clone())?;

    info!("New block accepted: {}", block_hash.dump_base36());
    let node_state = node_state.clone();

    // Forward to all peers (non blocking)
    tokio::spawn(async move {
        to_peers(
            Message::new(Command::NewBlock { block: new_block }),
            &node_state,
        )
        .await;
    });
    Ok(())
}

/// Accept a new block to the local blockchain, and forward it to all peers
pub async fn accept_transaction(
    blockchain: &SharedBlockchain,
    node_state: &SharedNodeState,
    new_transaction: Transaction,
) -> Result<(), BlockchainError> {
    new_transaction.check_completeness()?;
    let transaction_id = new_transaction.transaction_id.unwrap(); // Unwrap is okay, we checked that tx is complete

    if node_state.last_seen_transaction() == transaction_id {
        return Ok(()); // We already processed this tx
    }
    node_state.set_last_seen_transaction(transaction_id);

    // Validation
    blockchain::validate_transaction_timestamp(&new_transaction)?;
    blockchain.get_utxos().validate_transaction(
        &new_transaction,
        &BigUint::from_bytes_be(&blockchain.get_transaction_difficulty()),
    )?;

    node_state
        .mempool
        .add_transaction(new_transaction.clone())
        .await;

    info!("New transaction accepted: {}", transaction_id.dump_base36());
    let node_state = node_state.clone();

    // Forward to all peers (non blocking)
    tokio::spawn(async move {
        to_peers(
            Message::new(Command::NewTransaction {
                transaction: new_transaction,
            }),
            &node_state,
        )
        .await;
    });
    Ok(())
}
