use std::{collections::HashMap, sync::RwLock};

use crate::{
    core::block::BlockMetadata,
    crypto::{Hash, keys::Public},
};

pub struct InterestingBlocks {
    // Changed HashMap to use Public as key and Vec<Hash> as value
    pub interested: RwLock<HashMap<Public, Vec<Hash>>>,
    interested_addresses: Vec<Public>,
}

impl InterestingBlocks {
    pub fn new(interested_addresses: Vec<Public>) -> Self {
        Self {
            // Initialize with empty HashMap
            interested: RwLock::new(HashMap::new()),
            interested_addresses,
        }
    }

    pub fn scan_and_add(&self, block_meta: &BlockMetadata) {
        if let Some(block_hash) = block_meta.hash {
            // Find which addresses are present in this block
            for address in &self.interested_addresses {
                let is_present = block_meta
                    .address_inclusion_filter
                    .search_filter(*address)
                    .is_ok_and(|r| r);

                if is_present {
                    // Lock the map and append the hash to that specific address's list
                    let mut map = self.interested.write().unwrap();
                    map.entry(*address).or_insert_with(Vec::new).push(block_hash);
                }
            }
        }
    }

    // Returns all hashes found across all monitored addresses
    pub fn get_all_interested_hashes(&self) -> Vec<Hash> {
        let map = self.interested.read().unwrap();
        map.values().flatten().cloned().collect()
    }

    // Returns hashes for a specific address
    pub fn get_hashes_for_address(&self, address: &Public) -> Vec<Hash> {
        self.interested.read().unwrap().get(address).cloned().unwrap_or_default()
    }
}