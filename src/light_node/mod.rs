/// Handles Light Node State
pub mod light_node_state;

/// Handles storing downloaded block meta
pub mod block_meta_store;

/// Handles storing blocks that the node might be interested in
pub mod interesting_blocks;

/// Starts initial block download
pub mod initial_block_download;

/// Handles what the light node does when it gets a p2p message
mod behavior;

use flexi_logger::{Duplicate, FileSpec, Logger};
use log::info;
use num_bigint::BigUint;
use std::{
    net::SocketAddr,
    path::PathBuf,
    sync::{Arc, Once},
};
use tokio::net::TcpStream;

use crate::{
    core::{
        block::{Block, MAX_TRANSACTIONS_PER_BLOCK},
        blockchain::{self, BlockchainError},
        difficulty::calculate_block_difficulty,
        transaction::{MAX_TRANSACTION_IO, Transaction, TransactionError},
    },
    light_node::{
        behavior::LightNodePeerBehavior,
        light_node_state::{LightChainEvent, LightNodeState},
    },
    node::peer::{PeerError, PeerHandle, create_peer},
};

pub type SharedLightNodeState = Arc<LightNodeState>;

static LOGGER_INIT: Once = Once::new();

/// Creates a full node (SharedBlockchain and SharedNodeState), connecting to peers, accepting blocks and transactions
pub fn create_light_node(node_path: &str, disable_stdout: bool) -> SharedLightNodeState {
    let node_path = PathBuf::from(node_path);

    LOGGER_INIT.call_once(|| {
        let log_path = node_path.join("logs");
        std::fs::create_dir_all(&log_path).expect("Failed to create log directory");

        let mut logger = Logger::try_with_str("info")
            .unwrap()
            .log_to_file(FileSpec::default().directory(&log_path));

        if !disable_stdout {
            logger = logger.duplicate_to_stderr(Duplicate::Info);
        }

        logger.start().ok(); // Ignore errors if logger is already set

        info!("Logger initialized for node at {:?}", node_path);
    });

    let node_state = LightNodeState::new_empty(node_path);

    Arc::new(node_state)
}

/// Connect to a peer
pub async fn connect_peer(
    address: SocketAddr,
    light_node_state: &SharedLightNodeState,
) -> Result<PeerHandle, PeerError> {
    let stream = TcpStream::connect(address)
        .await
        .map_err(|e| PeerError::Io(format!("IO error: {e}")))?;

    let handle = create_peer(
        stream,
        LightNodePeerBehavior::new(light_node_state.clone()),
        false,
    )?;
    light_node_state
        .connected_peers
        .write()
        .await
        .insert(address, handle.clone());

    Ok(handle)
}

/// Accept a new block to the local blockchain, and forward it to all peers
pub async fn accept_block(
    light_node_state: &SharedLightNodeState,
    new_block: Block,
) -> Result<(), PeerError> {
    // Make sure merkle tree, filter, and hash
    if let Err(e) = new_block.check_meta() {
        return Err(BlockchainError::from(e).into());
    }
    let block_hash = new_block.meta.hash.unwrap(); // Unwrap is okay, we checked that block is complete

    // Validation
    blockchain::validate_block_timestamp(&new_block)?;
    for tx in &new_block.transactions {
        blockchain::validate_transaction_timestamp_in_block(tx, &new_block)?;
    }

    if light_node_state.meta_store().get_last_block_hash() != new_block.meta.previous_block {
        return Err(BlockchainError::InvalidPreviousBlockHash.into());
    }

    if new_block.transactions.len() > MAX_TRANSACTIONS_PER_BLOCK {
        return Err(BlockchainError::TooManyTransactions.into());
    }

    if BigUint::from_bytes_be(&*block_hash)
        > BigUint::from_bytes_be(&calculate_block_difficulty(
            &light_node_state
                .meta_store()
                .difficulty_state
                .get_block_difficulty(),
            new_block.transactions.len(),
        ))
    {
        return Err(
            BlockchainError::from(TransactionError::InsufficientDifficulty(
                block_hash.dump_base36(),
            ))
            .into(),
        );
    }

    if let Err(e) = new_block.validate_difficulties(
        &light_node_state
            .meta_store()
            .difficulty_state
            .get_block_difficulty(),
        &light_node_state
            .meta_store()
            .difficulty_state
            .get_transaction_difficulty(),
    ) {
        return Err(BlockchainError::from(e).into());
    }

    light_node_state
        .meta_store()
        .difficulty_state
        .update_difficulty(&new_block);

    light_node_state
        .meta_store()
        .save_block_meta(new_block.meta.clone())?;

    info!("New block accepted: {}", block_hash.dump_base36());

    // Broadcast new block
    let _ = light_node_state.chain_events.send(LightChainEvent::Block {
        block: new_block.clone(),
    });
    Ok(())
}

/// Accept a new block to the local blockchain, and forward it to all peers
pub async fn accept_transaction(
    light_node_state: &SharedLightNodeState,
    new_transaction: Transaction,
) -> Result<(), BlockchainError> {
    new_transaction.check_completeness()?;
    let transaction_id = new_transaction.transaction_id.unwrap(); // Unwrap is okay, we checked that tx is complete
    if light_node_state
        .seen_transactions
        .read()
        .await
        .contains(&transaction_id)
    {
        return Ok(());
    }
    light_node_state
        .seen_transactions
        .write()
        .await
        .insert(transaction_id);

    let transaction_hashing_buf = new_transaction
        .get_tx_hashing_buf()
        .map_err(|e| BlockchainError::BincodeEncode(e.to_string()))?;

    // Validation
    blockchain::validate_transaction_timestamp(&new_transaction)?;
    new_transaction.check_completeness()?;

    if !transaction_id.compare_with_data(&transaction_hashing_buf) {
        return Err(TransactionError::InvalidHash(transaction_id.dump_base36()).into());
    }

    if BigUint::from_bytes_be(&*transaction_id)
        > BigUint::from_bytes_be(
            &light_node_state
                .meta_store()
                .difficulty_state
                .get_transaction_difficulty(),
        )
    {
        return Err(TransactionError::InsufficientDifficulty(transaction_id.dump_base36()).into());
    }

    if new_transaction.inputs.len() + new_transaction.outputs.len() > MAX_TRANSACTION_IO {
        return Err(TransactionError::TooMuchIO.into());
    }

    if new_transaction.inputs.is_empty() {
        return Err(TransactionError::NoInputs.into());
    }

    for input in &new_transaction.inputs {
        if input.signature.is_none()
            || input
                .signature
                .unwrap()
                .validate_with_public(
                    &input.output_owner,
                    &new_transaction
                        .get_input_signing_buf()
                        .map_err(|e| BlockchainError::BincodeEncode(e.to_string()))?,
                )
                .map_or(true, |valid| !valid)
        {
            return Err(TransactionError::InvalidSignature(transaction_id.dump_base36()).into());
        }
    }

    // Broadcast new transaction
    let _ = light_node_state
        .chain_events
        .send(LightChainEvent::Transaction {
            transaction: new_transaction.clone(),
        });

    Ok(())
}
