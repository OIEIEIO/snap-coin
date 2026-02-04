use std::sync::{
    Arc, Mutex,
    atomic::{AtomicUsize, Ordering},
};
use std::time::{Duration, Instant};

use anyhow::anyhow;
use futures::{StreamExt, TryStreamExt, stream};
use log::info;
use tokio::task::spawn_blocking;

use crate::{
    core::block::Block,
    full_node::SharedBlockchain,
    node::{
        message::{Command, Message},
        peer::{PeerError, PeerHandle},
    },
};

const IBD_SAFE_SKIP_TX_HASHING: usize = 500;
const BUFFER_SIZE: usize = 10;
const STATUS_EVERY: usize = 1000;

#[derive(Default)]
struct IbdStats {
    downloaded: Duration,
    applied: Duration,
}

pub async fn ibd_blockchain(
    peer: PeerHandle,
    blockchain: SharedBlockchain,
    full_ibd: bool,
) -> Result<(), anyhow::Error> {
    info!("Starting initial block download");

    let local_height = blockchain.block_store().get_height();

    // ---- Fetch remote height ----
    let remote_height = match peer
        .request(Message::new(Command::Ping {
            height: local_height,
        }))
        .await?
        .command
    {
        Command::Pong { height } => height,
        _ => return Err(anyhow!("Could not fetch peer height to sync blockchain")),
    };

    if remote_height <= local_height {
        info!("[SYNC] Already synced");
        return Ok(());
    }

    // ---- Fetch block hashes ----
    let hashes = match peer
        .request(Message::new(Command::GetBlockHashes {
            start: local_height,
            end: remote_height,
        }))
        .await?
        .command
    {
        Command::GetBlockHashesResponse { block_hashes } => block_hashes,
        _ => {
            return Err(anyhow!(
                "Could not fetch peer block hashes to sync blockchain"
            ));
        }
    };

    info!("[SYNC] Fetched {} block hashes", hashes.len());

    let total = hashes.len();
    let left = Arc::new(AtomicUsize::new(total));
    let done = Arc::new(AtomicUsize::new(0));

    let stats = Arc::new(Mutex::new(IbdStats::default()));
    let start_time = Instant::now();

    stream::iter(hashes)
        .map(|hash| {
            let peer = peer.clone();
            let stats = stats.clone();

            async move {
                let dl_start = Instant::now();

                let resp = peer
                    .request(Message::new(Command::GetBlock { block_hash: hash }))
                    .await?;

                let block = match resp.command {
                    Command::GetBlockResponse { block } => block
                        .ok_or_else(|| anyhow!("Peer returned empty block {}", hash.dump_base36())),
                    _ => Err(anyhow!(
                        "Unexpected response for block {}",
                        hash.dump_base36()
                    )),
                }
                .map_err(|e| PeerError::Unknown(e.to_string()))?;

                stats.lock().unwrap().downloaded += dl_start.elapsed();

                Ok::<Block, PeerError>(block)
            }
        })
        .buffered(BUFFER_SIZE)
        .try_for_each(|block| {
            let stats = stats.clone();
            let blockchain = blockchain.clone();
            let left = left.clone();
            let done = done.clone();

            async move {
                let apply_start = Instant::now();

                let left_to_add = left.load(Ordering::SeqCst);
                spawn_blocking(move || blockchain.add_block(
                    block,
                    left_to_add > IBD_SAFE_SKIP_TX_HASHING && !full_ibd,
                )).await.map_err(|e| PeerError::Unknown(e.to_string()))??;

                stats.lock().unwrap().applied += apply_start.elapsed();

                left.fetch_sub(1, Ordering::SeqCst);
                let finished = done.fetch_add(1, Ordering::SeqCst) + 1;

                if finished % STATUS_EVERY == 0 || finished == total {
                    let elapsed = start_time.elapsed().as_secs_f64().max(0.001);
                    let speed = finished as f64 / elapsed;

                    let remaining = total - finished;
                    let eta_secs = (remaining as f64 / speed).max(0.0);

                    let pct = (finished as f64 / total as f64) * 100.0;

                    let stats = stats.lock().unwrap();
                    let bottleneck = if stats.downloaded > stats.applied {
                        "download"
                    } else {
                        "apply"
                    };

                    info!(
                        "[IBD] {:>6.2}% | {}/{} | {:>8.2} blk/s | left={} | ETA={}s | bottleneck={}",
                        pct,
                        finished,
                        total,
                        speed,
                        remaining,
                        eta_secs as u64,
                        bottleneck
                    );
                }

                Ok(())
            }
        })
        .await?;

    info!("[SYNC] Blockchain synced successfully");

    Ok(())
}
