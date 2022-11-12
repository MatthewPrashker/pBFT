use std::net::SocketAddr;

use serde::{Deserialize, Serialize};

use crate::{Key, NodeId, Value};

/// Messages which are communicated between nodes in the network
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Message {
    PrePrepareMessage(PrePrepare),
    PrepareMessage(Prepare),
    ClientRequestMessage(ClientRequest),
}

/// Commands to Consensus Engine
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ConsensusCommand {
    ProcessMessage(Message),
    MisdirectedClientRequest(ClientRequest),
    EnterPrePrepare(ClientRequest),
}

/// Commands to Node
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum NodeCommand {
    SendMessageCommand(SendMessage),
    BroadCastMessageCommand(BroadCastMessage),
}

// Messages

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PrePrepare {
    pub view: usize,
    pub seq_num: usize,
    pub digest: usize, /* This is going to be a hash of a client request */
    pub client_request: ClientRequest,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Prepare {
    pub view: usize,
    pub seq_num: usize,
    pub digest: usize, /* TODO: Make this some hash */
    pub id: NodeId,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ViewChange {
    pub new_view: usize,
    pub seq_num: usize,
    pub checkpoint_messages: Vec<Prepare>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub struct ClientRequest {
    pub respond_addr: SocketAddr,
    pub time_stamp: usize,
    pub key: Key,
    pub value: Value,
}

impl Message {
    pub fn serialize(&self) -> Vec<u8> {
        let mut serialized_message = serde_json::to_string(self).unwrap();
        serialized_message.push('\n');
        serialized_message.into_bytes()
    }
}

// Commands to Node

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SendMessage {
    pub destination: SocketAddr,
    pub message: Message,
}

// Commands to Node

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BroadCastMessage {
    pub message: Message,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct EnterCommit {
    pub view: usize,
    pub seq_num: usize,
    pub digest: usize, /* TODO: Make this some hash */
}
