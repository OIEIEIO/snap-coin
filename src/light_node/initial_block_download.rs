use futures::{stream, StreamExt};
use log::info;

use crate::{
    light_node::light_node_state::LightNodeState,
    node::{
        message::{Command, Message},
        peer::{PeerError, PeerHandle},
    },
};

const META_DOWNLOAD_BUFFER: usize = 8;

pub async fn initial_block_download(
    light_node_state: &LightNodeState,
    peer: &PeerHandle,
) -> Result<(), PeerError> {
    let local_height = light_node_state.meta_store().get_height();

    // Get remote height
    let remote_height = if let Command::Pong { height } = peer
        .request(Message::new(Command::Ping { height: local_height }))
        .await?
        .command
    {
        height
    } else {
        return Err(PeerError::IncorrectResponse);
    };

    if local_height >= remote_height {
        info!("Already up-to-date (local height >= remote height)");
        return Ok(());
    }

    info!("Starting initial block download (full)");

    // Fetch hashes
    let hashes = if let Command::GetBlockHashesResponse { block_hashes } = peer
        .request(Message::new(Command::GetBlockHashes {
            start: local_height,
            end: remote_height,
        }))
        .await?
        .command
    {
        block_hashes
    } else {
        return Err(PeerError::IncorrectResponse);
    };
    info!("Fetched {} block hashes", hashes.len());

    let meta_store = light_node_state.meta_store();

    info!("Downloading metadata concurrently");
    // Download all block metadata concurrently, preserving order
    let meta_s: Vec<_> = stream::iter(hashes.clone())
        .map(|block_hash| async move {
            if let Command::GetBlockMetadataResponse { block_metadata } =
                peer.request(Message::new(Command::GetBlockMetadata { block_hash })).await?.command
            {
                block_metadata.ok_or_else(|| {
                    PeerError::Unknown("Block metadata fetch returned None".into())
                })
            } else {
                Err(PeerError::IncorrectResponse)
            }
        })
        .buffered(META_DOWNLOAD_BUFFER)
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()?;

    info!("All metadata downloaded. Saving...");

    // Save metadata sequentially (they are in order)
    for meta in meta_s {
        meta_store
            .save_block_meta(meta)
            .map_err(|e| PeerError::Unknown(e.to_string()))?;
    }

    info!("Saved! Updating difficulties...");

    if let Some(last_hash) = hashes.last() {
        if let Command::GetBlockResponse { block } = peer.request(Message::new(Command::GetBlock { block_hash: *last_hash })).await?.command && let Some(block) = block {
            *light_node_state.meta_store().difficulty_state.block_difficulty.write().unwrap() = block.meta.block_pow_difficulty;
            *light_node_state.meta_store().difficulty_state.transaction_difficulty.write().unwrap() = block.meta.tx_pow_difficulty;
            light_node_state.meta_store().difficulty_state.update_difficulty(&block);
        }

    }

    info!("Initial block download complete");
    Ok(())
}
