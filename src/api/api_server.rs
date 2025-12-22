use std::sync::Arc;

use futures::io;
use thiserror::Error;
use tokio::{
    io::AsyncWriteExt,
    net::{TcpListener, TcpStream},
    sync::RwLock,
};

use crate::{
    api::requests::{Request, Response},
    blockchain_data_provider::BlockchainDataProvider,
    core::{blockchain::BlockchainError, transaction::TransactionError, utils::slice_vec},
    economics::get_block_reward,
    node::node::{Node, SharedBlockchain},
};

pub const PAGE_SIZE: u32 = 200;

#[derive(Error, Debug)]
pub enum ApiError {
    #[error("")]
    IOError(#[from] io::Error),
}

/// Server for hosting a Snap Coin API
pub struct Server {
    port: u32,
    blockchain: SharedBlockchain,
}

impl Server {
    /// Create a new server, do not listen for connections yet
    pub fn new(port: u32, blockchain: SharedBlockchain) -> Self {
        Server { port, blockchain }
    }

    /// Handle a incoming connection
    async fn connection(&self, mut stream: TcpStream) {
        loop {
            if let Err(e) = async {
                let request = Request::decode_from_stream(&mut stream).await?;
                let response = match request {
                    Request::Height => Response::Height {
                        height: self.blockchain.block_store().get_height() as u64,
                    },
                    Request::Block { block_hash } => Response::Block {
                        block: self.blockchain.block_store().get_block_by_hash(block_hash),
                    },
                    Request::BlockHash { height } => Response::BlockHash {
                        hash: self
                            .blockchain
                            .block_store()
                            .get_block_hash_by_height(height as usize),
                    },
                    Request::Transaction { transaction_id } => Response::Transaction {
                        transaction: self
                            .blockchain
                            .block_store()
                            .get_transaction(transaction_id),
                    },
                    Request::TransactionsOfAddress {
                        address,
                        page: requested_page,
                    } => {
                        let mut transactions = vec![];

                        for block in self.blockchain.block_store().iter_blocks() {
                            let block = block?;
                            for tx in block.transactions {
                                if tx.contains_address(address) {
                                    transactions.push(
                                        tx.transaction_id.ok_or(TransactionError::MissingId)?,
                                    );
                                }
                            }
                        }

                        let page = slice_vec(
                            &transactions,
                            (requested_page * PAGE_SIZE) as usize,
                            ((requested_page + 1) * PAGE_SIZE) as usize,
                        );
                        let next_page = if page.len() != PAGE_SIZE as usize {
                            None
                        } else {
                            Some(requested_page + 1)
                        };

                        Response::TransactionsOfAddress {
                            transactions: page.to_vec(),
                            next_page,
                        }
                    }
                    Request::AvailableUTXOs {
                        address,
                        page: requested_page,
                    } => {
                        let available = self
                            .blockchain
                            .get_available_transaction_outputs(address)
                            .await?;
                        let page = slice_vec(
                            &available,
                            (requested_page * PAGE_SIZE) as usize,
                            ((requested_page + 1) * PAGE_SIZE) as usize,
                        );
                        let next_page = if page.len() != PAGE_SIZE as usize {
                            None
                        } else {
                            Some(requested_page + 1)
                        };
                        Response::AvailableUTXOs {
                            available_inputs: page.to_vec(),
                            next_page,
                        }
                    }
                    Request::Balance { address } => Response::Balance {
                        balance: node
                            .read()
                            .await
                            .blockchain
                            .get_utxos()
                            .calculate_confirmed_balance(address),
                    },
                    Request::Reward => Response::Reward {
                        reward: get_block_reward(node.read().await.blockchain.get_height()),
                    },
                    Request::Peers => {
                        let node_guard = node.read().await;

                        let mut peers = vec![];
                        for peer in &node_guard.peers {
                            peers.push(peer.read().await.address);
                        }
                        Response::Peers { peers }
                    }
                    Request::Mempool {
                        page: requested_page,
                    } => {
                        let mempool = node.read().await.mempool.get_mempool().await;
                        let page = slice_vec(
                            &mempool,
                            (requested_page * PAGE_SIZE) as usize,
                            ((requested_page + 1) * PAGE_SIZE) as usize,
                        );
                        let next_page = if page.len() != PAGE_SIZE as usize {
                            None
                        } else {
                            Some(requested_page + 1)
                        };

                        Response::Mempool {
                            mempool: page.to_vec(),
                            next_page,
                        }
                    }
                    Request::NewBlock { new_block } => Response::NewBlock {
                        status: Node::submit_block(node.clone(), new_block).await,
                    },
                    Request::NewTransaction { new_transaction } => Response::NewTransaction {
                        status: Node::submit_transaction(node.clone(), new_transaction).await,
                    },
                    Request::Difficulty => Response::Difficulty {
                        transaction_difficulty: node
                            .read()
                            .await
                            .blockchain
                            .get_transaction_difficulty(),
                        block_difficulty: node.read().await.blockchain.get_block_difficulty(),
                    },
                    Request::BlockHeight { hash } => Response::BlockHeight {
                        height: node.read().await.blockchain.get_height_by_hash(&hash),
                    },
                };
                let response_buf = response.encode()?;

                stream.write_all(&response_buf).await?;

                Ok::<(), anyhow::Error>(())
            }
            .await
            {
                Node::log(format!("API client error: {}", e));
                break;
            }
        }
    }

    /// Start listening for clients
    pub async fn listen(&self) -> Result<(), ApiError> {
        let listener = match TcpListener::bind(format!("127.0.0.1:{}", self.port)).await {
            Ok(l) => l,
            Err(_) => TcpListener::bind("127.0.0.1:0").await?,
        };
        Node::log(format!(
            "API Server listening on 127.0.0.1:{}",
            listener.local_addr()?.port()
        ));
        let node = self.node.clone();
        tokio::spawn(async move {
            loop {
                if let Err(e) = async {
                    let (stream, _) = listener.accept().await?;

                    tokio::spawn(Server::connection(stream, node.clone()));

                    Ok::<(), ApiError>(())
                }
                .await
                {
                    Node::log(format!("API client failed to connect: {}", e))
                }
            }
        });

        Ok(())
    }
}
