//! # pool - vote tracking and certificate agregation
//! 
//! the pool is responsible for:
//! - collecting and storing votes from validators for each slot
//! - calculating stake-weighted vote tallies  
//! - creating certificates when vote thresholds are met
//! - emitting events when consensus milestones are reached
//!
//! ## certificate types:
//! - **NotarCertificate**: 60% of stake voted to notarize a block
//! - **FastFinalCertificate**: 80% of stake voted to notarize (immediate finality)
//! - **SkipCertificate**: 60% of stake voted to skip a slot
//!
//! ## key insight:
//! pool is "reactive" - it receives votes and creates certificates.
//! it doesn't decide WHEN to vote, only tracks votes and aggregates them.
//! stateless decisions: just the math (60% threshold = certificate)

use std::collections::HashMap;
use crate::types::*;
use log::{info, debug, warn};

#[derive(Debug, Clone)]
pub enum PoolEvent {
    BlockNotarized { slot: Slot, block_hash: Hash, certificate: NotarCertificate },
    FastFinalized { slot: Slot, block_hash: Hash, certificate: FastFinalCertificate },
    SlotSkipped { slot: Slot, certificate: SkipCertificate },
}

pub struct Pool {
    // track votes per slot: slot -> all votes for that slot
    votes: HashMap<Slot, SlotVotes>,
    
    // track certificates per slot to avoid duplicates
    certificates: HashMap<Slot, SlotCertificates>,
    
    // network configuration for stake calculations
    network_config: NetworkConfig,
    
    // events ready to be consumed
    pending_events: Vec<PoolEvent>,
}

#[derive(Default, Debug, Clone)]
struct SlotVotes {
    // validator_id -> their notarization vote  
    notar_votes: HashMap<ValidatorId, NotarVote>,
    
    // validator_id -> their skip vote
    skip_votes: HashMap<ValidatorId, SkipVote>,
    
    // validator_id -> their finalization vote
    final_votes: HashMap<ValidatorId, FinalVote>,
}

#[derive(Default, Debug)]
struct SlotCertificates {
    notar_cert: Option<NotarCertificate>,
    fast_final_cert: Option<FastFinalCertificate>,
    skip_cert: Option<SkipCertificate>,
}

enum CertificateAction {
    CreateFastFinal { slot: Slot, block_hash: Hash, votes: Vec<NotarVote>, stake: u64 },
    CreateNotar { slot: Slot, block_hash: Hash, votes: Vec<NotarVote>, stake: u64 },
    CreateSkip { slot: Slot, votes: Vec<SkipVote>, stake: u64 },
}

impl Pool {
    pub fn new(network_config: NetworkConfig) -> Self {
        info!("Initializing Pool with {} validators total_stake={}", 
              network_config.validators.len(), network_config.total_stake());
        
        Pool {
            votes: HashMap::new(),
            certificates: HashMap::new(),
            network_config,
            pending_events: Vec::new(),
        }
    }
    
    /// add a vote to the pool and check if new certificates can be created
    pub fn add_vote(&mut self, vote: AlpenglowMessage) -> Vec<PoolEvent> {
        let events = match vote {
            AlpenglowMessage::NotarVote(vote) => {
                debug!("Pool receiving NotarVote slot={} validator_id={} block_hash={}", 
                    vote.slot, vote.validator_id, format_hash(&vote.block_hash));
                self.add_notar_vote(vote)
            }
            AlpenglowMessage::SkipVote(vote) => {
                debug!("Pool receiving SkipVote slot={} validator_id={}", 
                    vote.slot, vote.validator_id);
                self.add_skip_vote(vote)
            }
            AlpenglowMessage::FinalVote(vote) => {
                debug!("Pool receiving FinalVote slot={} validator_id={}", 
                    vote.slot, vote.validator_id);
                self.add_final_vote(vote)
            }
            _ => {
                debug!("Pool ignoring non-vote message: {:?}", vote);
                vec![]
            }
        };
    
        
        // store events for later consumption
        self.pending_events.extend(events.clone());
        events
    }
    
    /// get pending events (for main loop consumption)
    pub fn drain_events(&mut self) -> Vec<PoolEvent> {
        std::mem::take(&mut self.pending_events)
    }
    
    /// check if a block has been notarized (60% threshold)
    pub fn is_block_notarized(&self, slot: Slot) -> bool {
        self.certificates.get(&slot)
            .map(|certs| certs.notar_cert.is_some())
            .unwrap_or(false)
    }
    
    /// check if a block has been fast-finalized (80% threshold)  
    pub fn is_block_fast_finalized(&self, slot: Slot) -> bool {
        self.certificates.get(&slot)
            .map(|certs| certs.fast_final_cert.is_some())
            .unwrap_or(false)
    }
    
    fn add_notar_vote(&mut self, vote: NotarVote) -> Vec<PoolEvent> {
        let slot = vote.slot;
        let validator_id = &vote.validator_id;
        
        debug!("Adding notarization vote slot={} validator_id={} block_hash={}", 
               slot, validator_id, format_hash(&vote.block_hash));
        
        let slot_votes = self.votes.entry(slot).or_default();
        
        // store the vote (replaces any previous notar vote from this node)
        slot_votes.notar_votes.insert(validator_id.clone(), vote);
        

        self.check_certificates(slot)
    }
    
    fn add_skip_vote(&mut self, vote: SkipVote) -> Vec<PoolEvent> {
        let slot = vote.slot;
        let validator_id = &vote.validator_id;
        
        debug!("Adding skip vote slot={} validator_id={}", slot, validator_id);
        
        let slot_votes = self.votes.entry(slot).or_default();
        slot_votes.skip_votes.insert(validator_id.clone(), vote);
        
        self.check_certificates(slot)
    }
    
    fn add_final_vote(&mut self, vote: FinalVote) -> Vec<PoolEvent> {
        let slot = vote.slot;
        let validator_id = &vote.validator_id;
        
        debug!("Adding finalization vote slot={} validator_id={}", slot, validator_id);
        
        let slot_votes = self.votes.entry(slot).or_default();
        slot_votes.final_votes.insert(validator_id.clone(), vote);
        
        // final votes don't create certificates directly, but might trigger events
        vec![]
    }
    
    fn check_certificates(&mut self, slot: Slot) -> Vec<PoolEvent> {
        let mut events = vec![];

        let Some(slot_votes) = self.votes.get(&slot).cloned() else {
            return events;
        };
        
        let certificate_actions = self.calculate_certificate_actions(slot, &slot_votes);
        
        for action in certificate_actions {
            match action {
                CertificateAction::CreateFastFinal { slot, block_hash, votes, stake } => {
                    let cert = FastFinalCertificate { slot, block_hash, votes, total_stake: stake };
                    self.certificates.entry(slot).or_default().fast_final_cert = Some(cert.clone());
                    events.push(PoolEvent::FastFinalized { slot, block_hash, certificate: cert });
                }
                CertificateAction::CreateNotar { slot, block_hash, votes, stake } => {
                    let cert = NotarCertificate { slot, block_hash, votes, total_stake: stake };
                    self.certificates.entry(slot).or_default().notar_cert = Some(cert.clone());
                    events.push(PoolEvent::BlockNotarized { slot, block_hash, certificate: cert });
                }
                CertificateAction::CreateSkip { slot, votes, stake } => {
                    let cert = SkipCertificate { slot, votes, total_stake: stake };
                    self.certificates.entry(slot).or_default().skip_cert = Some(cert.clone());
                    events.push(PoolEvent::SlotSkipped { slot, certificate: cert });
                }
            }
        }
        
        events
    }

    fn calculate_certificate_actions(&self, slot: Slot, slot_votes: &SlotVotes) -> Vec<CertificateAction> {
        let mut actions = vec![];
        let total_stake = self.network_config.total_stake();
        
        debug!("Pool checking certificates for slot={} total_stake={}", slot, total_stake);
        debug!("Slot {} has {} notar_votes, {} skip_votes", 
            slot, slot_votes.notar_votes.len(), slot_votes.skip_votes.len());
        
        // group votes by block hash
        let mut notar_by_block: HashMap<Hash, Vec<NotarVote>> = HashMap::new();
        for vote in slot_votes.notar_votes.values() {
            notar_by_block.entry(vote.block_hash).or_default().push(vote.clone());
        }
        
        debug!("Slot {} has {} different blocks with votes", slot, notar_by_block.len());
        
        // check thresholds
        for (block_hash, votes) in notar_by_block {
            let stake: u64 = votes.iter()
                .map(|v| self.network_config.get_stake(&v.validator_id))
                .sum();
            let percentage = (stake * 100) / total_stake;
            
            debug!("Slot {} block {} has {} votes with {}% stake ({}/{})", 
                slot, format_hash(&block_hash), votes.len(), percentage, stake, total_stake);
            
            if percentage >= 80 && !self.certificates.get(&slot).map_or(false, |c| c.fast_final_cert.is_some()) {
                debug!("Creating Fast-Finalization Certificate for slot={} ({}%)", slot, percentage);
                actions.push(CertificateAction::CreateFastFinal { slot, block_hash, votes, stake: total_stake });
            } else if percentage >= 60 && !self.certificates.get(&slot).map_or(false, |c| c.notar_cert.is_some()) {
                debug!("Creating Notarization Certificate for slot={} ({}%)", slot, percentage);
                actions.push(CertificateAction::CreateNotar { slot, block_hash, votes, stake: total_stake });
            }
        }
        
        debug!("Pool generated {} certificate actions for slot={}", actions.len(), slot);
        actions
    }
}

fn format_hash(hash: &Hash) -> String {
    hex::encode(&hash[..4])
}