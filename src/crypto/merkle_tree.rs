use crate::core::transaction::TransactionId;
use bincode::{Decode, Encode};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// A node in the Merkle tree (of transaction ids)
#[derive(Debug, Clone, Serialize, Deserialize, Encode, Decode)]
pub struct MerkleNode {
    pub hash: [u8; 32],
    pub left: Option<Box<MerkleNode>>,
    pub right: Option<Box<MerkleNode>>,
}

/// A Merkle tree
#[derive(Debug, Clone, Serialize, Deserialize, Encode, Decode)]
pub struct MerkleTree {
    pub root: MerkleNode,
}

impl MerkleTree {
    /// Build a Merkle tree from a list of transactions
    pub fn build(transactions: &[TransactionId]) -> MerkleTree {
        let mut leaves: Vec<MerkleNode> = transactions
            .iter()
            .map(|tx| {
                let mut hasher = Sha256::new();
                hasher.update(&tx.dump_buf());
                MerkleNode {
                    hash: hasher.finalize().into(),
                    left: None,
                    right: None,
                }
            })
            .collect();

        if leaves.is_empty() {
            return MerkleTree {
                root: MerkleNode {
                    hash: [0u8; 32],
                    left: None,
                    right: None,
                },
            };
        }

        while leaves.len() > 1 {
            let mut next_level = vec![];
            for i in (0..leaves.len()).step_by(2) {
                let left = leaves[i].clone();
                let right = if i + 1 < leaves.len() {
                    leaves[i + 1].clone()
                } else {
                    left.clone()
                };

                let mut hasher = Sha256::new();
                hasher.update(&left.hash);
                hasher.update(&right.hash);
                let parent_hash: [u8; 32] = hasher.finalize().into();

                next_level.push(MerkleNode {
                    hash: parent_hash,
                    left: Some(Box::new(left)),
                    right: Some(Box::new(right)),
                });
            }
            leaves = next_level;
        }

        Self {
            root: leaves[0].clone(),
        }
    }

    /// Get the Merkle root hash
    pub fn root_hash(&self) -> [u8; 32] {
        self.root.hash
    }

    /// Verify inclusion of a transaction hash
    pub fn verify_inclusion(&self, tx_hash: [u8; 32]) -> bool {
        fn verify(node: &MerkleNode, tx_hash: [u8; 32]) -> bool {
            if node.left.is_none() && node.right.is_none() {
                return node.hash == tx_hash;
            }
            node.left.as_ref().map_or(false, |l| verify(l, tx_hash))
                || node.right.as_ref().map_or(false, |r| verify(r, tx_hash))
        }
        verify(&self.root, tx_hash)
    }

    /// Generate Merkle proof (with left/right sibling info)
    pub fn generate_proof(&self, tx_hash: [u8; 32]) -> Option<MerkleTreeProof> {
        fn helper(node: &MerkleNode, tx_hash: [u8; 32], path: &mut Vec<([u8; 32], bool)>) -> bool {
            if node.left.is_none() && node.right.is_none() {
                return node.hash == tx_hash;
            }

            if let Some(left) = &node.left {
                if helper(left, tx_hash, path) {
                    let right_hash = node.right.as_ref().unwrap().hash;
                    path.push((right_hash, false)); // sibling is right
                    return true;
                }
            }

            if let Some(right) = &node.right {
                if helper(right, tx_hash, path) {
                    let left_hash = node.left.as_ref().unwrap().hash;
                    path.push((left_hash, true)); // sibling is left
                    return true;
                }
            }

            false
        }

        let mut path = Vec::new();
        if helper(&self.root, tx_hash, &mut path) {
            Some(MerkleTreeProof { tx_hash, path })
        } else {
            None
        }
    }
}

/// Merkle inclusion proof
#[derive(Debug, Clone, Serialize, Deserialize, Encode, Decode)]
pub struct MerkleTreeProof {
    pub tx_hash: [u8; 32],
    /// Vec of (sibling_hash, is_left_sibling)
    pub path: Vec<([u8; 32], bool)>,
}

impl MerkleTreeProof {
    /// Validate a Merkle proof
    pub fn validate(&self, root_hash: [u8; 32]) -> bool {
        let mut hash = self.tx_hash;
        for (sibling, is_left) in &self.path {
            let mut hasher = Sha256::new();
            if *is_left {
                hasher.update(sibling);
                hasher.update(&hash);
            } else {
                hasher.update(&hash);
                hasher.update(sibling);
            }
            hash = hasher.finalize().into();
        }
        hash == root_hash
    }

    /// Create a proof from a flat list of transaction hashes
    pub fn create_proof(tx_hashes: &[[u8; 32]], target_tx: [u8; 32]) -> Option<Self> {
        let mut index = tx_hashes.iter().position(|h| *h == target_tx)?;
        let mut layer = tx_hashes.to_vec();
        let mut path = Vec::new();

        while layer.len() > 1 {
            if layer.len() % 2 != 0 {
                layer.push(*layer.last().unwrap());
            }

            let sibling_index = if index % 2 == 0 { index + 1 } else { index - 1 };
            path.push((layer[sibling_index], index % 2 != 0)); // is_left if node is right

            // Build next layer
            let mut next_layer = Vec::with_capacity(layer.len() / 2);
            for pair in layer.chunks(2) {
                let mut hasher = Sha256::new();
                hasher.update(&pair[0]);
                hasher.update(&pair[1]);
                next_layer.push(hasher.finalize().into());
            }
            layer = next_layer;
            index /= 2;
        }

        Some(MerkleTreeProof { tx_hash: target_tx, path })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Dummy TransactionId generator
    fn dummy_tx_id(byte: u8) -> TransactionId {
        TransactionId::new_from_buf([byte; 32])
    }

    #[test]
    fn test_merkle_tree_basic() {
        let tx_ids: Vec<TransactionId> = (1..=5).map(dummy_tx_id).collect();
        let tree = MerkleTree::build(&tx_ids);

        assert_ne!(tree.root_hash(), [0u8; 32]);

        for tx in &tx_ids {
            let mut hasher = Sha256::new();
            hasher.update(&tx.dump_buf());
            let hash: [u8; 32] = hasher.finalize().into();
            assert!(tree.verify_inclusion(hash));
        }

        let fake = dummy_tx_id(255);
        let mut hasher = Sha256::new();
        hasher.update(&fake.dump_buf());
        let hash: [u8; 32] = hasher.finalize().into();
        assert!(!tree.verify_inclusion(hash));
    }

    #[test]
    fn test_merkle_proof() {
        let tx_ids: Vec<TransactionId> = (10..14).map(dummy_tx_id).collect();
        let tree = MerkleTree::build(&tx_ids);

        for tx in &tx_ids {
            let mut hasher = Sha256::new();
            hasher.update(&tx.dump_buf());
            let hash: [u8; 32] = hasher.finalize().into();

            let proof = tree.generate_proof(hash).unwrap();
            assert!(proof.validate(tree.root_hash()));
        }

        let fake = dummy_tx_id(99);
        let mut hasher = Sha256::new();
        hasher.update(&fake.dump_buf());
        let hash: [u8; 32] = hasher.finalize().into();
        assert!(tree.generate_proof(hash).is_none());
    }

    #[test]
    fn test_merkle_tree_proof_creation() {
        let tx_ids: Vec<TransactionId> = (20..25).map(dummy_tx_id).collect();
        let tx_hashes: Vec<[u8; 32]> = tx_ids
            .iter()
            .map(|tx| {
                let mut hasher = Sha256::new();
                hasher.update(&tx.dump_buf());
                hasher.finalize().into()
            })
            .collect();

        let target = tx_hashes[3];
        let proof = MerkleTreeProof::create_proof(&tx_hashes, target).unwrap();

        // compute root manually
        let mut layer = tx_hashes.clone();
        while layer.len() > 1 {
            if layer.len() % 2 != 0 {
                layer.push(*layer.last().unwrap());
            }
            layer = layer
                .chunks(2)
                .map(|pair| {
                    let mut hasher = Sha256::new();
                    hasher.update(&pair[0]);
                    hasher.update(&pair[1]);
                    hasher.finalize().into()
                })
                .collect();
        }
        let root = layer[0];

        assert!(proof.validate(root));

        let mut bad = proof.clone();
        bad.tx_hash[0] ^= 0xFF;
        assert!(!bad.validate(root));
    }
}
