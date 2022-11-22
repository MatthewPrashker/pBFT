/// Byzantine Fault Tolerant KV-Store

pub type NodeId = usize;

pub type Key = String;
pub type Value = u32;

pub mod config;
pub mod consensus;
pub mod message_bank;
pub mod messages;
pub mod node;
pub mod state;
pub mod view_changer;

pub use sha2::{Digest, Sha256};

pub type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

