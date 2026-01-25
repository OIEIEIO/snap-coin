use serde::{Deserialize, Serialize};
use std::{
    collections::{HashMap, VecDeque},
    net::SocketAddr,
    sync::Arc,
};
use tokio::sync::{
    Mutex, RwLock, broadcast,
    watch::{self, Ref},
};

use crate::{
    core::{
        block::Block,
        difficulty::calculate_live_transaction_difficulty,
        transaction::{Transaction, TransactionId},
    },
    crypto::Hash,
    full_node::mempool::MemPool,
    node::peer::PeerHandle,
};

pub type SharedNodeState = Arc<NodeState>;

pub struct NodeState {
    pub connected_peers: RwLock<HashMap<SocketAddr, PeerHandle>>,
    pub mempool: MemPool,
    pub is_syncing: RwLock<bool>,
    pub chain_events: broadcast::Sender<ChainEvent>,
    pub adding_block: Mutex<()>,
    last_seen_block_reader: watch::Receiver<Hash>,
    last_seen_block_writer: watch::Sender<Hash>,
    last_seen_transactions_reader: watch::Receiver<VecDeque<TransactionId>>,
    last_seen_transactions_writer: watch::Sender<VecDeque<TransactionId>>,
}

impl NodeState {
    pub fn new_empty() -> SharedNodeState {
        let (last_seen_block_writer, last_seen_block_reader) =
            watch::channel(Hash::new_from_buf([0u8; 32]));
        let (last_seen_transactions_writer, last_seen_transactions_reader) =
            watch::channel(VecDeque::new());

        Arc::new(NodeState {
            connected_peers: RwLock::new(HashMap::new()),
            mempool: MemPool::new(),
            is_syncing: RwLock::new(false),
            chain_events: broadcast::channel(64).0,
            adding_block: Mutex::new(()),
            last_seen_block_reader,
            last_seen_block_writer,
            last_seen_transactions_reader,
            last_seen_transactions_writer,
        })
    }

    /// Get the latest seen block
    pub fn last_seen_block(&self) -> Hash {
        self.last_seen_block_reader.borrow().clone()
    }

    /// Set a new last seen block
    pub fn set_last_seen_block(&self, hash: Hash) {
        let _ = self.last_seen_block_writer.send(hash);
    }

    /// Get the latest seen transactions
    pub fn last_seen_transactions(&self) -> Ref<'_, VecDeque<TransactionId>> {
        self.last_seen_transactions_reader.borrow()
    }

    /// Add a new last seen transaction, removing the oldest if >500
    pub fn add_last_seen_transaction(&self, tx_id: TransactionId) {
        let mut transactions: VecDeque<TransactionId> =
            self.last_seen_transactions_reader.borrow().clone();

        // Avoid duplicates
        if !transactions.contains(&tx_id) {
            transactions.push_back(tx_id);

            // Keep only the latest 500 transactions
            if transactions.len() > 500 {
                transactions.pop_front(); // remove oldest
            }

            let _ = self.last_seen_transactions_writer.send(transactions);
        }
    }

    pub async fn get_live_transaction_difficulty(
        &self,
        transaction_difficulty: [u8; 32],
    ) -> [u8; 32] {
        calculate_live_transaction_difficulty(
            &transaction_difficulty,
            self.mempool.mempool_size().await,
        )
    }
}

#[derive(Serialize, Deserialize, Clone)]
pub enum ChainEvent {
    Block { block: Block },
    Transaction { transaction: Transaction },
    TransactionExpiration { transaction: TransactionId },
}
