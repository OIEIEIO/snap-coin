use bincode::{Decode, Encode};
use probabilistic_collections::{SipHasherBuilder, bloom::BloomFilter};
use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Error, Debug, Clone, Serialize, Deserialize, Encode, Decode)]
pub enum AddressInclusionFilterError {
    #[error("Failed to encode bloom filter")]
    BloomEncoding,

    #[error("Failed to decode bloom filter")]
    BloomDecoding,
}

use crate::{core::transaction::Transaction, crypto::keys::Public};

const FALSE_POSITIVE_RATE: f64 = 0.001;

#[derive(Debug, Clone, Serialize, Deserialize, Encode, Decode, PartialEq)]
pub struct AddressInclusionFilter {
    bloom: Vec<u8>,
}

impl AddressInclusionFilter {
    pub fn create_filter(
        transactions: &[Transaction],
    ) -> Result<Self, AddressInclusionFilterError> {
        let address_count = transactions
            .iter()
            .fold(0, |acc, tx| acc + tx.address_count());

        let mut bloom: BloomFilter<[u8; 32]> = BloomFilter::with_hashers(
            address_count,
            FALSE_POSITIVE_RATE,
            [
                SipHasherBuilder::from_seed(0, 0),
                SipHasherBuilder::from_seed(0, 0),
            ],
        );

        for tx in transactions {
            Self::insert_transaction(&mut bloom, tx);
        }

        // TODO: Serialize into something way smarter than JSON
        let bloom =
            serde_json::to_vec(&bloom).map_err(|_| AddressInclusionFilterError::BloomEncoding)?;

        Ok(Self { bloom })
    }

    pub fn search_filter(&self, address: Public) -> Result<bool, AddressInclusionFilterError> {
        let bloom = serde_json::from_slice::<BloomFilter<[u8; 32]>>(&self.bloom)
            .map_err(|_| AddressInclusionFilterError::BloomDecoding)?;
        Ok(bloom.contains(&*address))
    }

    fn insert_transaction(bloom: &mut BloomFilter<[u8; 32]>, tx: &Transaction) {
        for input in &tx.inputs {
            bloom.insert(&*input.output_owner);
        }

        for output in &tx.outputs {
            bloom.insert(&*output.receiver);
        }
    }
}
