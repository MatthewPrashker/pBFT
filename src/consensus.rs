use crate::config::Config;
use crate::messages::{
    BroadCastMessage, ClientRequest, Commit, ConsensusCommand, Message, NodeCommand, PrePrepare,
    Prepare, SendMessage
};
use crate::{NodeId, Key, Value};

use tokio::sync::mpsc::{Receiver, Sender};
use tokio::sync::Mutex;
use tokio::time::sleep;

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::Arc;

// Note that all communication between the Node and the Consensus engine takes place
// by the outer consensus struct

pub struct Consensus {
    /// Id of the current node
    pub id: NodeId,
    /// Configuration of the cluster this node is in
    pub config: Config,
    /// Receiver of Consensus Commands
    pub rx_consensus: Receiver<ConsensusCommand>,
    /// Sends Commands to Node
    pub tx_node: Sender<NodeCommand>,
    /// Inner part of Consensus moving between tasks
    pub inner: InnerConsensus,
}

#[derive(Clone)]
pub struct InnerConsensus {
    /// Id of the current node
    pub id: NodeId,
    /// Configuration of the cluster this node is in
    pub config: Config,
    /// Send Consensus Commands back to the outer consensus engine
    pub tx_consensus: Sender<ConsensusCommand>,
    /// Maps (view, seq_num) to client request seen for that pair
    /// A request is inserted once we accept a pre-prepare message from the network for this request
    pub requests_seen: Arc<Mutex<HashMap<(usize, usize), ClientRequest>>>,

    pub prepare_votes: Arc<Mutex<HashMap<(usize, usize), HashSet<NodeId>>>>,

    pub commit_votes: Arc<Mutex<HashMap<(usize, usize), HashSet<NodeId>>>>,
    /// These are added when we either get a misdirected client request 
    /// or we accept a pre-prepare message
    /// Used to initiate view changes
    pub outstanding_requests: Arc<Mutex<HashSet<ClientRequest>>>,
    /// Current state of conensus
    pub state: Arc<Mutex<State>>,
}

#[derive(Default)]
pub struct State {
    pub in_view_change: bool,
    pub view: usize,
    pub seq_num: usize,
    pub log: VecDeque<Message>,
    pub store: HashMap<Key, Value>,
}

impl Consensus {
    pub fn new(
        id: NodeId,
        config: Config,
        rx_consensus: Receiver<ConsensusCommand>,
        tx_consensus: Sender<ConsensusCommand>,
        tx_node: Sender<NodeCommand>,
    ) -> Self {
        let inner_consensus = InnerConsensus {
            id,
            config: config.clone(),
            tx_consensus,
            requests_seen: Arc::new(Mutex::new(HashMap::new())),
            prepare_votes: Arc::new(Mutex::new(HashMap::new())),
            commit_votes: Arc::new(Mutex::new(HashMap::new())),
            outstanding_requests: Arc::new(Mutex::new(HashSet::new())),
            state: Arc::new(Mutex::new(State::default())),
        };

        Self {
            id,
            config,
            rx_consensus,
            tx_node,
            inner: inner_consensus,
        }
    }

    pub fn current_leader(&self, state: &State) -> NodeId {
        state.view % self.config.num_nodes
    }

    pub async fn spawn(&mut self) {
        loop {
            tokio::select! {
                //main future listening for incoming commands
                res = self.rx_consensus.recv() => {
                    let cmd = res.unwrap();
                    println!("Consensus Engine Received Command {:?}", cmd);
                    match cmd {
                        ConsensusCommand::ProcessMessage(message) => {
                            let mut inner = self.inner.clone();
                            tokio::spawn(async move {
                                inner.process_message(&message).await;
                            });
                        }

                        ConsensusCommand::MisdirectedClientRequest(request) => {
                            // If we get a client request but are not the leader
                            // we forward the request to the leader. We started a timer
                            // which, if it expires and the request is still outstanding,
                            // will initiate the view change protocol
                            let state = self.inner.state.lock().await;
                            self.inner.add_outstanding_request(&request).await;
                            
                            let leader = self.current_leader(&state);
                            let leader_addr = self.config.peer_addrs.get(&leader).unwrap();
                            let _ = self.tx_node.send(NodeCommand::SendMessageCommand(SendMessage {
                                destination: *leader_addr,
                                message: Message::ClientRequestMessage(request.clone()),
                            })).await;


                            let inner = self.inner.clone();
                            tokio::spawn(async move {
                                inner.wait_for_outstanding(&request.clone()).await;
                            });
                        }

                        ConsensusCommand::ProcessClientRequest(request) => {
                            let state = self.inner.state.lock().await;
                            
                            if self.id != self.current_leader(&state){
                                let _ = self.inner
                                    .tx_consensus
                                    .send(ConsensusCommand::MisdirectedClientRequest(request.clone()))
                                    .await;
                            } else {
                            // at this point we are the leader and we have accepted a client request
                            // which we may begin to process
                            let _ = self.inner
                                .tx_consensus
                                .send(ConsensusCommand::InitPrePrepare(request.clone()))
                                .await;
                            }
                        }

                        ConsensusCommand::InitPrePrepare(request) => {
                            // Here we are primary and received a client request which we deemed valid
                            // so we broadcast a Pre_prepare Message to the network

                            let state = self.inner.state.lock().await;
                            let pre_prepare = PrePrepare {
                                id: self.id,
                                view: state.view,
                                seq_num: state.seq_num,
                                digest: request.clone().hash(),
                                signature: 0,
                                client_request: request,
                            };
                            let pre_prepare_message = Message::PrePrepareMessage(pre_prepare.clone());

                            let _ = self.tx_node.send(NodeCommand::BroadCastMessageCommand(BroadCastMessage {
                                message: pre_prepare_message.clone()
                            })).await;

                        }

                        ConsensusCommand::AcceptPrePrepare(pre_prepare) => {
                            // We received a PrePrepare message from the network, and we see no violations
                            // So we will broadcast a corresponding prepare message and begin to count votes
                            let mut state = self.inner.state.lock().await;
                            let mut requests_seen = self.inner.requests_seen.lock().await;

                            self.inner.add_outstanding_request(&pre_prepare.client_request).await;
                            // at this point, we need to trigger a timer, and if the timer expires 
                            // and the request is still outstanding, then we need to trigger a view change
                            // as this is evidence that the system has stopped making progress
                            requests_seen.insert((state.view, state.seq_num), pre_prepare.client_request.clone());

                            let prepare = Prepare {
                                id: self.id,
                                view: state.view,
                                seq_num: state.seq_num,
                                digest: pre_prepare.clone().digest,
                                signature: 0,
                            };

                            let prepare_message = Message::PrepareMessage(prepare.clone());
                            let _ = self.tx_node.send(NodeCommand::BroadCastMessageCommand(BroadCastMessage {
                                message: prepare_message.clone(),
                            })).await;


                            state.log.push_back(Message::PrePrepareMessage(pre_prepare.clone()));
                            state.log.push_back(prepare_message);

                            let inner = self.inner.clone();
                            tokio::spawn(async move {
                                inner.wait_for_outstanding(&pre_prepare.client_request).await;
                            });
                        }

                        ConsensusCommand::AcceptPrepare(prepare) => {
                            // We saw a prepare message from the network that we deemed was valid
                            // So we increment the vote count, and if we have enough prepare votes
                            // Then we move to the commit phases
                            let mut state = self.inner.state.lock().await;
                            let mut prepare_votes = self.inner.prepare_votes.lock().await;


                            state.log.push_back(Message::PrepareMessage(prepare.clone()));

                            if let Some(curr_vote_set) = prepare_votes.get_mut(&(prepare.view, prepare.seq_num)) {
                                curr_vote_set.insert(prepare.id);
                                if curr_vote_set.len() > 2*self.config.num_faulty {
                                    // at this point, we have enough prepare votes to move into the commit phase.
                                    let _ = self.inner.tx_consensus.send(ConsensusCommand::EnterCommit(prepare)).await;
                                }
                            } else {
                                // first time we got a prepare message for this view and sequence number
                                let mut new_vote_set = HashSet::new();
                                new_vote_set.insert(prepare.id);
                                prepare_votes.insert((prepare.view, prepare.seq_num), new_vote_set);
                            }
                        }

                        ConsensusCommand::EnterCommit(prepare) => {
                            let state = self.inner.state.lock().await;
                            
                            println!("BEGINNING COMMIT PHASE");
                            let commit = Commit {
                                id: self.id,
                                view: state.view,
                                seq_num: state.seq_num,
                                digest: prepare.digest,
                                signature: 0,
                            };
                            let commit_message = Message::CommitMessage(commit);
                            let _ = self.tx_node.send(NodeCommand::BroadCastMessageCommand(BroadCastMessage {
                                message: commit_message,
                            })).await;
                        }

                        ConsensusCommand::AcceptCommit(commit) => {
                            // We received a Commit Message for a request that we deemed valid
                            // so we increment the vote count

                            let mut state = self.inner.state.lock().await;
                            let mut commit_votes = self.inner.commit_votes.lock().await;


                            state.log.push_back(Message::CommitMessage(commit.clone()));

                            if let Some(curr_vote_set) = commit_votes.get_mut(&(commit.view, commit.seq_num)) {
                                curr_vote_set.insert(commit.id);
                                if curr_vote_set.len() > 2*self.config.num_faulty {
                                    // At this point, we have enough commit votes to commit the message
                                    let _ = self.inner.tx_consensus.send(ConsensusCommand::ApplyClientRequest(commit)).await;
                                }
                            } else {
                                // first time we got a prepare message for this view and sequence number
                                let mut new_vote_set = HashSet::new();
                                new_vote_set.insert(commit.id);
                                commit_votes.insert((commit.view, commit.seq_num), new_vote_set);
                            }
                        }

                        ConsensusCommand::InitViewChange(request) => {
                            let mut state = self.inner.state.lock().await;
                            if state.in_view_change || self.current_leader(&state) == self.id {
                                // we are already in a view change state or we are currently the leader
                                return;
                            }
                            println!("Initializing view change...");
                            state.in_view_change = true;
                        }

                        ConsensusCommand::ApplyClientRequest(commit) => {
                            // we now have permission to apply the client request
                            let mut state = self.inner.state.lock().await;
                            let requests_seen = self.inner.requests_seen.lock().await;
                            //remove the request from the outstanding requests so that we can trigger the view change
                            let client_request = requests_seen.get(&(commit.view, commit.seq_num)).unwrap();
                            self.inner.remove_outstanding_request(client_request).await;

                            if client_request.value.is_some() {
                                // request is a set request
                                state.store.insert(client_request.clone().key, client_request.clone().value.unwrap());
                            } {
                                //request is a get request
                            }

                            state.seq_num += 1;

                        }
                    }
                }
            }
        }
    }
}

impl InnerConsensus {
    pub async fn process_message(&mut self, message: &Message) {
        // Note that we should not grab any locks here
        match message.clone() {
            Message::PrePrepareMessage(pre_prepare) => {
                if self.should_accept_pre_prepare(&pre_prepare).await {
                    let _ = self
                        .tx_consensus
                        .send(ConsensusCommand::AcceptPrePrepare(pre_prepare))
                        .await;
                }
            }
            Message::PrepareMessage(prepare) => {
                if self.should_accept_prepare(&prepare).await {
                    let _ = self
                        .tx_consensus
                        .send(ConsensusCommand::AcceptPrepare(prepare))
                        .await;
                }
            }
            Message::CommitMessage(commit) => {
                if self.should_accept_commit(&commit).await {
                    let _ = self
                        .tx_consensus
                        .send(ConsensusCommand::AcceptCommit(commit))
                        .await;
                }
            }
            Message::ClientRequestMessage(client_request) => {
                if self.should_process_client_request(&client_request).await {
                    let _ = self.tx_consensus.send(ConsensusCommand::ProcessClientRequest(client_request)).await;
                }
            }
            Message::ClientResponseMessage(_) => {
                // we should never receive a client response message
            }
        }
    }

    async fn add_outstanding_request(&self, request: &ClientRequest) {
        let mut outstanding_requests = self.outstanding_requests.lock().await;
        outstanding_requests.insert(request.clone());
    }

    async fn remove_outstanding_request(&self, request: &ClientRequest) {
        let mut outstanding_requests = self.outstanding_requests.lock().await;
        outstanding_requests.remove(request);
    }

    async fn request_is_outstanding(&self, request: &ClientRequest) -> bool {
        let outstanding_requests = self.outstanding_requests.lock().await;
        outstanding_requests.contains(request)
    }

    async fn in_view_change(&self) -> bool {
        let state = self.state.lock().await;
        state.in_view_change
    }

    async fn should_accept_pre_prepare(&self, message: &PrePrepare) -> bool {
        let state = self.state.lock().await;
        if state.view != message.view {
            return false;
        }
        // verify that the digest of the message is equal to the hash of the client_request
        // check if we have already seen a sequence number for this view
        // have we accepted a pre-prepare message in this view with same sequence number and different digest
        true
    }

    async fn should_accept_prepare(&self, message: &Prepare) -> bool {
        let state = self.state.lock().await;
        let requests_seen = self.requests_seen.lock().await;

        if state.view != message.view {
            return false;
        }

        // make sure we already saw a request with given view and sequence number,
        // and make sure that the digests are correct.
        if let Some(e_pre_prepare) = requests_seen.get(&(message.view, message.seq_num)) {
            if message.digest != *e_pre_prepare.hash() {
                return false;
            }
        } else {
            // we have not seen a pre_prepare message for any request
            // with this given (view, seq_num) pair, so we cannot accept a prepare
            // for this request
            return false;
        }
        true
    }

    async fn should_accept_commit(&self, messsage: &Commit) -> bool {
        true
    }

    async fn should_process_client_request(&mut self, request: &ClientRequest) -> bool {
        if self.in_view_change().await {
            // if we are in the view change state
            // then we do not process any client requests
            return false;
        }
        true
    }

    async fn wait_for_outstanding(&self, request: &ClientRequest) {
        sleep(std::time::Duration::from_secs(5)).await;
        if self.request_is_outstanding(&request.clone()).await {
            let _ = self.tx_consensus.send(ConsensusCommand::InitViewChange(request.clone())).await;
        }
    }
}
