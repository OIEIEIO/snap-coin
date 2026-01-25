use serde::{Deserialize, Serialize};
use tokio::sync::{RwLock, broadcast};

use crate::{
    bounded_set::BoundedSet,
    core::{
        block::Block,
        transaction::{Transaction, TransactionId},
    },
    crypto::Hash,
    light_node::block_meta_store::BlockMetaStore,
    node::peer::PeerHandle,
};
use std::{collections::HashMap, net::SocketAddr, path::PathBuf};

pub struct LightNodeState {
    pub chain_events: broadcast::Sender<LightChainEvent>,
    pub connected_peers: RwLock<HashMap<SocketAddr, PeerHandle>>,
    pub seen_transactions: RwLock<BoundedSet<TransactionId>>,
    pub seen_blocks: RwLock<BoundedSet<Hash>>,
    meta_store: BlockMetaStore,
}

impl LightNodeState {
    pub fn new_empty(node_path: PathBuf) -> Self {
        Self {
            connected_peers: RwLock::new(HashMap::new()),
            meta_store: BlockMetaStore::new(node_path),
            chain_events: broadcast::channel(12).0,
            seen_transactions: RwLock::new(BoundedSet::new(1000)),
            seen_blocks: RwLock::new(BoundedSet::new(100)),
        }
    }
    pub fn meta_store(&self) -> &BlockMetaStore {
        &self.meta_store
    }
}

#[derive(Serialize, Deserialize, Clone)]
pub enum LightChainEvent {
    Block { block: Block },
    Transaction { transaction: Transaction },
}
