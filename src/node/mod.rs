/// Handles node creation (SharedBlockchain and SharedNodeState), connecting to peers, accepting blocks and transactions
pub mod node;

/// Stores all currently pending transactions, that are waiting to be mined
pub mod mempool;

/// Stores current node state, shared between threads
pub mod node_state;

/// Encodes and Decodes P2P messages
pub mod message;

/// Handles peer creation, with a PeerHandle struct for handling peers
pub mod peer;

/// Handles network on message logic
mod on_message;

/// Enforces longest chain rule, syncs to a peer that has a higher height
mod sync;

/// Listens for incoming peer connections
pub mod server;

/// Handles public node discovery, and connection. A daemon
pub mod auto_peer;