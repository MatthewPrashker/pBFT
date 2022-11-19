use crate::messages::{ClientRequest, Commit, Message, PrePrepare, Prepare};
use crate::NodeId;

use std::collections::{HashMap, HashSet, VecDeque};

#[derive(Default)]
pub struct MessageBank {
    /// The log of accepted messages
    pub log: VecDeque<Message>,
    pub accepted_pre_prepare_requests: HashMap<(usize, usize), PrePrepare>,
    /// Valid prepares that we received that we did not accept
    pub outstanding_prepares: HashSet<Prepare>,
    /// Valid commits that we received that we did not accept
    pub outstanding_commits: HashSet<Commit>,
    /// commits we accepted but did not apply
    pub accepted_commits_not_applied: HashMap<usize, Commit>,

    pub applied_commits: HashMap<usize, (Commit, ClientRequest)>,
}
