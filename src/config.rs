use std::collections::HashMap;
use std::hash::Hash;
use std::net::SocketAddr;

use crate::NodeId;

#[derive(Clone)]
pub struct Config {
    /// Number of nodes in the system
    pub num_nodes: usize,
    /// Address which each node is listening on
    pub listen_addrs: HashMap<NodeId, SocketAddr>,
}
