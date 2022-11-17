use crate::config::Config;
use crate::messages::{
    BroadCastMessage, Commit, ConsensusCommand, Message, NodeCommand, PrePrepare, Prepare,
    SendMessage,
};
use crate::state::State;
use crate::view_changer::ViewChanger;
use crate::NodeId;

use tokio::sync::mpsc::{Receiver, Sender};

use std::collections::HashSet;
use std::sync::{Arc, Mutex};

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
    /// Sends Consensus Commands to itself
    pub tx_consensus: Sender<ConsensusCommand>,
    /// Current State of the Consensus
    pub state: State,
    /// Responsible for outstanding requests and changing views
    pub view_changer: ViewChanger,
}

impl Consensus {
    pub fn new(
        id: NodeId,
        config: Config,
        rx_consensus: Receiver<ConsensusCommand>,
        tx_consensus: Sender<ConsensusCommand>,
        tx_node: Sender<NodeCommand>,
    ) -> Self {
        let state = State {
            config: config.clone(),
            ..Default::default()
        };

        let view_changer = ViewChanger {
            id,
            config: config.clone(),
            tx_consensus: tx_consensus.clone(),
            wait_set: Arc::new(Mutex::new(HashSet::new())),
        };

        Self {
            id,
            config,
            rx_consensus,
            tx_node,
            tx_consensus,
            state,
            view_changer,
        }
    }

    pub async fn spawn(&mut self) {
        loop {
            let res = self.rx_consensus.recv().await;
            let cmd = res.unwrap();
            //println!("Consensus Engine Received Command {:?}", cmd);
            match cmd {
                ConsensusCommand::ProcessMessage(message) => {
                    match message.clone() {
                        Message::IdentifierMessage(_) => {unreachable!()}
                        
                        Message::PrePrepareMessage(pre_prepare) => {
                            println!("Saw preprepare from {}", pre_prepare.id);
                            if self.state.should_accept_pre_prepare(&pre_prepare) {
                                let _ = self
                                    .tx_consensus
                                    .send(ConsensusCommand::AcceptPrePrepare(pre_prepare))
                                    .await;
                            }
                        }
                        Message::PrepareMessage(prepare) => {
                            println!("Saw prepare from {}", prepare.id);
                            if self.state.should_accept_prepare(&prepare) {
                                let _ = self
                                    .tx_consensus
                                    .send(ConsensusCommand::AcceptPrepare(prepare))
                                    .await;
                            } else {
                                self.state
                                    .message_bank
                                    .outstanding_prepares
                                    .insert(prepare.clone());
                            }
                        }
                        Message::CommitMessage(commit) => {
                            println!("Saw commit from {}", commit.id);
                            if self.state.should_accept_commit(&commit) {
                                let _ = self
                                    .tx_consensus
                                    .send(ConsensusCommand::AcceptCommit(commit))
                                    .await;
                            } else {
                                self.state
                                    .message_bank
                                    .outstanding_commits
                                    .insert(commit.clone());
                            }
                        }
                        Message::ClientRequestMessage(client_request) => {
                            if self.state.should_process_client_request(&client_request) {
                                if self.id != self.state.current_leader() {
                                    let _ = self
                                        .tx_consensus
                                        .send(ConsensusCommand::MisdirectedClientRequest(
                                            client_request.clone(),
                                        ))
                                        .await;
                                } else {
                                    // at this point we are the leader and we have accepted a client request
                                    // which we may begin to process
                                    let _ = self
                                        .tx_consensus
                                        .send(ConsensusCommand::InitPrePrepare(
                                            client_request.clone(),
                                        ))
                                        .await;
                                }
                            }
                        }

                        Message::ClientResponseMessage(_) => {
                            // we should never receive a client response message
                            unreachable!()
                        }
                    }
                }

                ConsensusCommand::MisdirectedClientRequest(request) => {
                    // If we get a client request but are not the leader
                    // we forward the request to the leader. We started a timer
                    // which, if it expires and the request is still outstanding,
                    // will initiate the view change protocol

                    let leader = self.state.current_leader();
                    let leader_addr = self.config.peer_addrs.get(&leader).unwrap();
                    let _ = self
                        .tx_node
                        .send(NodeCommand::SendMessageCommand(SendMessage {
                            destination: *leader_addr,
                            message: Message::ClientRequestMessage(request.clone()),
                        }))
                        .await;

                    // if we are adding
                    let newly_added = self.view_changer.add_to_wait_set(&request);
                    if newly_added {
                        let view_changer = self.view_changer.clone();
                        tokio::spawn(async move {
                            view_changer.wait_for(&request.clone()).await;
                        });
                    }
                }

                ConsensusCommand::InitPrePrepare(request) => {
                    // Here we are primary and received a client request which we deemed valid
                    // so we broadcast a Pre_prepare Message to the network and assign
                    // the next sequence number to this request
                    self.state.seq_num += 1;

                    let pre_prepare = PrePrepare {
                        id: self.id,
                        view: self.state.view,
                        seq_num: self.state.seq_num,
                        digest: request.clone().hash(),
                        signature: 0,
                        client_request: request,
                    };
                    let pre_prepare_message = Message::PrePrepareMessage(pre_prepare.clone());

                    let _ = self
                        .tx_node
                        .send(NodeCommand::BroadCastMessageCommand(BroadCastMessage {
                            message: pre_prepare_message.clone(),
                        }))
                        .await;
                }

                ConsensusCommand::AcceptPrePrepare(pre_prepare) => {
                    // We received a PrePrepare message from the network, and we see no violations
                    // So we will broadcast a corresponding prepare message and begin to count votes

                    self.state.message_bank.accepted_prepare_requests.insert(
                        (pre_prepare.view, pre_prepare.seq_num),
                        pre_prepare.client_request.clone(),
                    );

                    let prepare = Prepare {
                        id: self.id,
                        view: self.state.view,
                        seq_num: pre_prepare.seq_num,
                        digest: pre_prepare.clone().digest,
                        signature: 0,
                    };

                    let prepare_message = Message::PrepareMessage(prepare.clone());
                    let _ = self
                        .tx_node
                        .send(NodeCommand::BroadCastMessageCommand(BroadCastMessage {
                            message: prepare_message.clone(),
                        }))
                        .await;

                    self.state
                        .message_bank
                        .log
                        .push_back(Message::PrePrepareMessage(pre_prepare.clone()));

                    // we may already have a got a prepare message which we did not accept because
                    // we did not receive this pre-prepare message message yet
                    for e_prepare in self.state.message_bank.outstanding_prepares.iter() {
                        if e_prepare.corresponds_to(&pre_prepare) {
                            println!("Found outstanding prepare from {}", e_prepare.id);
                            let _ = self
                                .tx_consensus
                                .send(ConsensusCommand::AcceptPrepare(e_prepare.clone()))
                                .await;
                        }
                    }

                    // at this point, we need to trigger a timer, and if the timer expires
                    // and the request is still outstanding, then we need to trigger a view change
                    // as this is evidence that the system has stopped making progress
                    let newly_added = self
                        .view_changer
                        .add_to_wait_set(&pre_prepare.client_request);
                    if newly_added {
                        let view_changer = self.view_changer.clone();
                        tokio::spawn(async move {
                            view_changer.wait_for(&pre_prepare.client_request).await;
                        });
                    }
                }

                ConsensusCommand::AcceptPrepare(prepare) => {
                    // We saw a prepare message from the network that we deemed was valid
                    // to we increment the vote count, and if we have enough prepare votes
                    // then we move to the commit phases

                    println!("Accepted Prepare from {}", prepare.id);

                    // we are not accepting this prepare, so if it is our outstanding set, then
                    //we may remove it
                    self.state
                        .message_bank
                        .outstanding_prepares
                        .remove(&prepare);

                    // add the prepare message we are accepting to the log
                    self.state
                        .message_bank
                        .log
                        .push_back(Message::PrepareMessage(prepare.clone()));

                    // TODO: Move the prepare votes into the state struct
                    // Count votes for this prepare message and see if we have enough to move to the commit phases
                    if let Some(curr_vote_set) = self
                        .state
                        .prepare_votes
                        .get_mut(&(prepare.view, prepare.seq_num))
                    {
                        curr_vote_set.insert(prepare.id);
                        if curr_vote_set.len() > 2 * self.config.num_faulty {
                            // at this point, we have enough prepare votes to move into the commit phase.
                            let _ = self
                                .view_changer
                                .tx_consensus
                                .send(ConsensusCommand::EnterCommit(prepare.clone()))
                                .await;
                        }
                    } else {
                        // first time we got a prepare message for this view and sequence number
                        let mut new_vote_set = HashSet::new();
                        new_vote_set.insert(prepare.id);
                        self.state
                            .prepare_votes
                            .insert((prepare.view, prepare.seq_num), new_vote_set);
                    }

                    // we may already have a got a commit message which we did not accept because
                    // we did not receive this prepare message message yet
                    for e_commit in self.state.message_bank.outstanding_commits.iter() {
                        if e_commit.corresponds_to(&prepare) {
                            println!("Found outstanding commit from {}", e_commit.id);
                            let _ = self
                                .tx_consensus
                                .send(ConsensusCommand::AcceptCommit(e_commit.clone()))
                                .await;
                        }
                    }
                }

                ConsensusCommand::AcceptCommit(commit) => {
                    // We received a Commit Message for a request that we deemed valid
                    // so we increment the vote count

                    println!("Accepted commit from {}", commit.id);

                    self.state.message_bank.outstanding_commits.remove(&commit);

                    self.state
                        .message_bank
                        .log
                        .push_back(Message::CommitMessage(commit.clone()));

                    if let Some(curr_vote_set) = self
                        .state
                        .commit_votes
                        .get_mut(&(commit.view, commit.seq_num))
                    {
                        curr_vote_set.insert(commit.id);
                        if curr_vote_set.len() > 2 * self.config.num_faulty {
                            // At this point, we have enough commit votes to commit the message
                            let _ = self
                                .tx_consensus
                                .send(ConsensusCommand::ApplyClientRequest(commit))
                                .await;
                        }
                    } else {
                        // first time we got a prepare message for this view and sequence number
                        let mut new_vote_set = HashSet::new();
                        new_vote_set.insert(commit.id);
                        self.state
                            .commit_votes
                            .insert((commit.view, commit.seq_num), new_vote_set);
                    }
                }

                ConsensusCommand::EnterCommit(prepare) => {
                    println!("BEGINNING COMMIT PHASE");
                    let commit = Commit {
                        id: self.id,
                        view: self.state.view,
                        seq_num: prepare.seq_num,
                        digest: prepare.digest,
                        signature: 0,
                    };
                    let commit_message = Message::CommitMessage(commit);
                    let _ = self
                        .tx_node
                        .send(NodeCommand::BroadCastMessageCommand(BroadCastMessage {
                            message: commit_message,
                        }))
                        .await;
                }

                ConsensusCommand::InitViewChange(request) => {
                    if self.state.in_view_change || self.state.current_leader() == self.id {
                        // we are already in a view change state or we are currently the leader
                        return;
                    }
                    println!("Initializing view change...");
                    self.state.in_view_change = true;
                }

                ConsensusCommand::ApplyClientRequest(commit) => {
                    // we now have permission to apply the client request

                    let client_request = self
                        .state
                        .message_bank
                        .accepted_prepare_requests
                        .get(&(commit.view, commit.seq_num))
                        .unwrap()
                        .clone();

                    // remove this request from the view changer so that we don't trigger a view change
                    self.view_changer.remove_from_wait_set(&client_request);

                    println!("Applying client request with seq_num {}", commit.seq_num);
                    self.state.apply_commit(&client_request, &commit);

                    // The request we just committed was enough to now trigger a checkpoint
                    if self.state.last_seq_num_committed % self.config.checkpoint_frequency == 0 {
                        //trigger the checkpoint process
                    }
                }
            }
        }
    }
}
