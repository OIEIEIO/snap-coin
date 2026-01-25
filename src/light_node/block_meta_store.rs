use std::{
    collections::HashMap,
    fs::{self, File},
    io::Write,
    path::PathBuf,
    sync::{Mutex, RwLock},
};

use bincode::{Decode, Encode};
use thiserror::Error;

use crate::{
    core::{block::BlockMetadata, difficulty::DifficultyState},
    crypto::Hash,
    economics::GENESIS_PREVIOUS_BLOCK_HASH,
};

#[derive(Error, Debug, Clone)]
pub enum BlockMetaStoreError {
    #[error("Previous block hash does not match the last added block")]
    IncorrectPreviousBlock,

    #[error("Encoding failed")]
    Encode,

    #[error("Decoding failed")]
    Decode,

    #[error("IO error: {0}")]
    Io(String),
}

impl From<std::io::Error> for BlockMetaStoreError {
    fn from(e: std::io::Error) -> Self {
        BlockMetaStoreError::Io(e.to_string())
    }
}

#[derive(Encode, Decode, Clone)]
pub struct BlockMetaIndex {
    by_hash: HashMap<Hash, usize>,
    by_height: HashMap<usize, Hash>,
}

impl BlockMetaIndex {
    pub fn new_empty() -> Self {
        Self {
            by_hash: HashMap::new(),
            by_height: HashMap::new(),
        }
    }
}

#[derive(Encode, Decode)]
pub struct BlockMetaStore {
    pub difficulty_state: DifficultyState,
    node_path: PathBuf,
    last_block: RwLock<Hash>,
    meta_index: RwLock<BlockMetaIndex>,
    height: RwLock<usize>,
    adding_block: Mutex<()>
}

impl BlockMetaStore {
    pub fn new(node_path: PathBuf) -> Self {
        fs::create_dir_all(&node_path).ok();

        match Self::load_meta_store_data(&node_path) {
            Ok(block_meta_store) => block_meta_store,
            Err(_) => Self {
                difficulty_state: DifficultyState::new_default(),
                node_path,
                last_block: RwLock::new(GENESIS_PREVIOUS_BLOCK_HASH),
                meta_index: RwLock::new(BlockMetaIndex::new_empty()),
                height: RwLock::new(0),
                adding_block: Mutex::new(())
            },
        }
    }

    /// Saves block metadata to disk and updates indices
    pub fn save_block_meta(&self, block_meta: BlockMetadata) -> Result<(), BlockMetaStoreError> {
        let _add_guard = self.adding_block.lock().unwrap();
        // Enforce chain continuity
        if block_meta.previous_block != *self.last_block.read().unwrap() {
            return Err(BlockMetaStoreError::IncorrectPreviousBlock);
        }

        let height = *self.height.read().unwrap();
        let final_path = self.meta_path_by_height(height);
        let tmp_path = final_path.with_extension("dat.tmp");

        // Serialize
        let buffer = bincode::encode_to_vec(&block_meta, bincode::config::standard())
            .map_err(|_| BlockMetaStoreError::Encode)?;

        // Atomic write
        {
            let mut f = File::create(&tmp_path)?;
            f.write_all(&buffer)?;
            f.sync_all()?;
        }
        fs::rename(&tmp_path, &final_path)?;

        let hash = block_meta.hash.expect("BlockMetadata must contain hash");

        // Update index
        {
            let mut index = self.meta_index.write().unwrap();
            index.by_hash.insert(hash, height);
            index.by_height.insert(height, hash);
        }

        // Update height + last block
        *self.height.write().unwrap() = height + 1;
        *self.last_block.write().unwrap() = hash;

        self.save_meta_store_data()?;

        Ok(())
    }

    pub fn get_height(&self) -> usize {
        *self.height.read().unwrap()
    }

    pub fn get_last_block_hash(&self) -> Hash {
        *self.last_block.read().unwrap()
    }

    pub fn get_meta_by_height(&self, height: usize) -> Option<BlockMetadata> {
        let path = self.meta_path_by_height(height);
        let data = fs::read(path).ok()?;
        let (meta, _) = bincode::decode_from_slice(&data, bincode::config::standard()).ok()?;
        Some(meta)
    }

    pub fn get_meta_by_hash(&self, hash: Hash) -> Option<BlockMetadata> {
        let height = *self.meta_index.read().unwrap().by_hash.get(&hash)?;
        self.get_meta_by_height(height)
    }

    fn meta_path_by_height(&self, height: usize) -> PathBuf {
        self.node_path.join(format!("meta-{}.dat", height))
    }

    fn save_meta_store_data(&self) -> Result<(), BlockMetaStoreError> {
        let mut file = File::create(format!(
            "{}/meta-store.dat",
            self.node_path.to_str().unwrap()
        ))
        .map_err(|e| BlockMetaStoreError::Io(e.to_string()))?;

        bincode::encode_into_std_write(self.clone(), &mut file, bincode::config::standard())
            .map_err(|_| BlockMetaStoreError::Encode)?;

        file.sync_all()
            .map_err(|e| BlockMetaStoreError::Io(e.to_string()))?;
        Ok(())
    }

    fn load_meta_store_data(node_path: &PathBuf) -> Result<Self, BlockMetaStoreError> {
        let mut file = File::open(format!("{}/meta-store.dat", node_path.to_str().unwrap()))
            .map_err(|e| BlockMetaStoreError::Io(e.to_string()))?;
        Ok(
            bincode::decode_from_std_read(&mut file, bincode::config::standard())
                .map_err(|_| BlockMetaStoreError::Decode)?,
        )
    }
}

impl Clone for BlockMetaStore {
    fn clone(&self) -> Self {
        Self {
            node_path: self.node_path.clone(),
            last_block: RwLock::new(*self.last_block.read().unwrap()),
            meta_index: RwLock::new(self.meta_index.read().unwrap().clone()),
            height: RwLock::new(*self.height.read().unwrap()),
            difficulty_state: self.difficulty_state.clone(),
            adding_block: Mutex::new(())
        }
    }
}
