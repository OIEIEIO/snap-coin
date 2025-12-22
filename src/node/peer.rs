use std::{collections::HashMap, net::SocketAddr, sync::Arc, time::Duration};

use log::{error};
use tokio::{
    net::{
        TcpStream,
        tcp::{OwnedReadHalf, OwnedWriteHalf},
    },
    sync::{
        Mutex,
        mpsc::{self, Receiver},
        oneshot,
    },
    time::{sleep, timeout},
};

use thiserror::Error;

use crate::{
    core::blockchain::BlockchainError,
    node::{
        message::{Command, Message, MessageId},
        node::SharedBlockchain,
        node_state::SharedNodeState,
        on_message::on_message,
        sync::SyncError,
    },
};

/// Message expecting a response OR not
pub enum Outgoing {
    Request(Message, oneshot::Sender<Message>),
    OneWay(Message),
}

type Pending = Arc<Mutex<HashMap<MessageId, oneshot::Sender<Message>>>>;
type KillSignal = String;

/// Peer timeout, in seconds
pub const PEER_TIMEOUT: Duration = Duration::from_secs(5);

/// Peer ping interval, in seconds
pub const PEER_PING_INTERVAL: Duration = Duration::from_secs(5);

#[derive(Error, Debug)]
pub enum PeerError {
    #[error("IO error: {0}")]
    Io(String),

    #[error("Timeout waiting for peer response")]
    Timeout,

    #[error("Failed to send request to peer: {0}")]
    SendError(String),

    #[error("Failed to receive response from peer: {0}")]
    ReceiveError(String),

    #[error("Peer killed: {0}")]
    Killed(String),

    #[error("Message decoding error: {0}")]
    MessageDecode(String),

    #[error("Message encoding error: {0}")]
    MessageEncode(String),

    #[error("Peer disconnected unexpectedly")]
    Disconnected,

    #[error("Unknown error: {0}")]
    Unknown(String),

    #[error("Blockchain error: {0}")]
    Blockchain(#[from] BlockchainError),

    #[error("Sync error: {0}")]
    SyncError(String),
}

impl From<SyncError> for PeerError {
    fn from(sync_error: SyncError) -> Self {
        Self::SendError(sync_error.to_string())
    }
}

/// Used to reference, request, and kill
#[derive(Clone, Debug)]
pub struct PeerHandle {
    pub send: mpsc::Sender<Outgoing>,
    pub address: SocketAddr,
    pub is_client: bool,
    kill: Arc<Mutex<Option<oneshot::Sender<KillSignal>>>>,
}

/// Create a new peer, start internal tasks, and return a PeerHandle
pub fn create_peer(
    stream: TcpStream,
    blockchain: SharedBlockchain,
    node_state: SharedNodeState,
    is_client: bool,
) -> Result<PeerHandle, PeerError> {
    let address = stream
        .peer_addr()
        .map_err(|e| PeerError::Io(format!("IO error: {e}")))?;

    let (outgoing_tx, outgoing_rx) = mpsc::channel::<Outgoing>(64);
    let (kill, should_kill) = oneshot::channel::<KillSignal>();

    let handle = PeerHandle {
        send: outgoing_tx,
        kill: Arc::new(Mutex::new(Some(kill))),
        is_client,
        address,
    };
    let my_handle = handle.clone();

    tokio::spawn(async move {
        let node_state_fail = node_state.clone();
        if let Err(e) = async move {
            let (reader, writer) = stream.into_split();

            let pending: Pending =
                Arc::new(Mutex::new(HashMap::<MessageId, oneshot::Sender<Message>>::new()));

            tokio::select! {
                res = reader_task(reader, pending.clone(), my_handle.clone(), blockchain.clone(), node_state) => res,
                res = writer_task(writer, outgoing_rx, pending) => res,
                res = pinger_task(my_handle, blockchain) => res,
                res = async move {
                    let message = should_kill
                        .await
                        .map_err(|_| PeerError::Killed("Kill channel closed".to_string()))?;
                    Err(PeerError::Killed(message))
                } => res
            }?;

            Ok::<(), PeerError>(())
        }
        .await
        {
            tokio::spawn(async move {
                node_state_fail
                    .connected_peers
                    .write()
                    .await
                    .remove(&address);
                error!("Peer error (disconnected): {e}");
            });
            
        }
    });

    Ok(handle)
}

/// Send a request message, and expect a response message from this peer
pub async fn request_from_peer(
    peer: &PeerHandle,
    request: Message,
) -> Result<Message, PeerError> {
    let (callback_tx, callback_rx) = oneshot::channel::<Message>();

    match timeout(
        PEER_TIMEOUT,
        peer.send.send(Outgoing::Request(request, callback_tx)),
    )
    .await
    {
        Ok(res) => res.map_err(|e| PeerError::SendError(e.to_string()))?,
        Err(_) => {
            kill_peer(peer, "Peer timed out".to_string()).await?;
            return Err(PeerError::Timeout);
        }
    }

    callback_rx
        .await
        .map_err(|e| PeerError::ReceiveError(e.to_string()))
}

/// Send a message without expecting a response
pub async fn send_to_peer(peer: &PeerHandle, message: Message) -> Result<(), PeerError> {
    peer.send
        .send(Outgoing::OneWay(message))
        .await
        .map_err(|e| PeerError::SendError(e.to_string()))
}

/// Send a kill signal to this peer
pub async fn kill_peer(peer: &PeerHandle, message: String) -> Result<(), PeerError> {
    if let Some(kill) = peer.kill.lock().await.take() {
        kill.send(message.clone())
            .map_err(|_| PeerError::Killed(message))?;
    }
    Ok(())
}

async fn reader_task(
    mut stream: OwnedReadHalf,
    pending: Pending,
    my_handle: PeerHandle,
    blockchain: SharedBlockchain,
    node_state: SharedNodeState,
) -> Result<(), PeerError> {
    loop {
        let message = Message::from_stream(&mut stream)
            .await
            .map_err(|e| PeerError::MessageDecode(e.to_string()))?;

        if let Some(requester) = pending.lock().await.remove(&message.id) {
            let _ = requester.send(message);
        } else {
            let response = on_message(message, &my_handle, &blockchain, &node_state).await?;
            send_to_peer(&my_handle, response).await?;
        }
    }
}

async fn writer_task(
    mut stream: OwnedWriteHalf,
    mut receiver: Receiver<Outgoing>,
    pending: Pending,
) -> Result<(), PeerError> {
    while let Some(outgoing) = receiver.recv().await {
        match outgoing {
            Outgoing::Request(msg, responder) => {
                pending.lock().await.insert(msg.id, responder);
                msg.send(&mut stream)
                    .await
                    .map_err(|e| PeerError::MessageEncode(e.to_string()))?;
            }
            Outgoing::OneWay(msg) => {
                msg.send(&mut stream)
                    .await
                    .map_err(|e| PeerError::MessageEncode(e.to_string()))?;
            }
        }
    }
    Err(PeerError::Disconnected)
}

async fn pinger_task(
    my_handle: PeerHandle,
    blockchain: SharedBlockchain,
) -> Result<(), PeerError> {
    loop {
        sleep(PEER_PING_INTERVAL).await;
        request_from_peer(
            &my_handle,
            Message::new(Command::Ping {
                height: blockchain.block_store().get_height(),
            }),
        )
        .await?;
    }
}
