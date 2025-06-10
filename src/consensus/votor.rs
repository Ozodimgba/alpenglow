//! # votor - alpenglow voting state machine
//!
//! votor implements the core alpenglow consensus voting logic. It decides:
//! - WHEN to cast votes (NotarVote, SkipVote, FinalVote)  
//! - WHAT to vote for (which blocks to support)
//! - HOW to react to certificates and timeouts
//!
//! ## alpenglow's two Path xonsensus:
//! 1. **fast Path (80%)**: if a block gets 80% notarization votes → immediate finality
//! 2. **slow Path (60% → 60%)**: if a block gets 60% notar votes → second round with final votes
//!
//! ## key Responsibilities:
//! - vote on blocks when they arrive and pass validation
//! - vote to skip slots when leaders are slow/offline
//! - cast finalization votes in the second round
//! - handle timeout-based decisions  
//!
//! ## votor vs pool:
//! - **votor**: decides WHEN and WHAT to vote for (decision maker)
//! - **pool**: tracks votes and creates certificates (vote counter)


use std::collections::HashMap;
use tokio::time::{Duration, Instant};
use crate::types::*;
use crate::consensus::{Pool, PoolEvent};
use log::{info, debug, warn};

#[derive(Debug, Clone)]
pub enum VotorEvent {
    ShouldCastVote { vote: AlpenglowMessage },
    BlockFinalized { slot: Slot, block_hash: Hash },
    SlotSkipped { slot: Slot },
}

pub struct Votor {
    validator_id: ValidatorId,
    network_config: NetworkConfig,
    
    keypair: KeyPair,  
    // Track our voting state per slot
    slot_states: HashMap<Slot, SlotState>,
    
    // Track timeouts for slot progression
    slot_timeouts: HashMap<Slot, Instant>,
    
    // Events ready to be consumed
    pending_events: Vec<VotorEvent>,
}

#[derive(Debug, Clone)]
struct SlotState {
    // What we've voted for in this slot
    cast_notar_vote: Option<Hash>, // block hash we voted to notarize
    cast_skip_vote: bool,// whether we voted to skip
    cast_final_vote: bool, // whether we cast finalization vote
    
    // What we've observed
    received_block: Option<Hash>,// block hash we received
    block_notarized: bool, // whether any block got notarized (60%)
    block_fast_finalized: bool, // whether any block got fast-finalized (80%)
    slot_skipped: bool, // whether slot got skip certificate
    
    timeout_set: Option<Instant>,
}

impl Default for SlotState {
    fn default() -> Self {
        SlotState {
            cast_notar_vote: None,
            cast_skip_vote: false,
            cast_final_vote: false,
            received_block: None,
            block_notarized: false,
            block_fast_finalized: false,
            slot_skipped: false,
            timeout_set: None,
        }
    }
}

impl Votor {
    pub fn new(validator_id: ValidatorId, network_config: NetworkConfig) -> Self {
        info!("Initializing Votor validator_id={}", validator_id);

        // generate keypair for this validator
        let keypair = KeyPair::new(validator_id.clone());
        
        Votor {
            validator_id,
            network_config,
            keypair,
            slot_states: HashMap::new(),
            slot_timeouts: HashMap::new(),
            pending_events: Vec::new(),
        }
    }
    
    /// react to a block being received - decide whether to vote for it
    pub fn handle_block_received(&mut self, slot: Slot, block_hash: Hash) -> Vec<VotorEvent> {
        debug!("Votor handling block received slot={} block_hash={}", slot, format_hash(&block_hash));
        
        let mut events = vec![];

        let vote_data = self.create_vote_data(slot, block_hash);
        let signature = self.keypair.sign(&vote_data);

        let state = self.slot_states.entry(slot).or_default();
        
        state.received_block = Some(block_hash);

         debug!("Votor decision for slot={}: cast_notar_vote={:?} cast_skip_vote={} received_block={:?}", 
           slot, state.cast_notar_vote, state.cast_skip_vote, state.received_block);
        
        // decide whether to vote for this block
        let should_vote = state.cast_notar_vote.is_none() 
            && !state.cast_skip_vote 
            && !state.block_fast_finalized 
            && !state.slot_skipped
            && state.received_block == Some(block_hash);

        if should_vote {

            let vote = NotarVote {
                slot,
                block_hash,
                validator_id: self.validator_id.clone(),
                signature,
            };
            
            state.cast_notar_vote = Some(block_hash);
            
            info!("Votor casting notarization vote slot={} block_hash={}", slot, format_hash(&block_hash));
            events.push(VotorEvent::ShouldCastVote { 
                vote: AlpenglowMessage::NotarVote(vote) 
            });
        }
        
        self.pending_events.extend(events.clone());
        events
    }

    // helper method to create consistent vote data for signing
    fn create_vote_data(&self, slot: Slot, block_hash: Hash) -> Vec<u8> {
        let mut vote_data = Vec::new();
        vote_data.extend_from_slice(&slot.to_le_bytes());
        vote_data.extend_from_slice(&block_hash);
        vote_data.extend_from_slice(self.validator_id.as_bytes());
        vote_data
    }
    
    /// r\eact to Pool events - certificates being created
    pub fn handle_pool_event(&mut self, event: PoolEvent) -> Vec<VotorEvent> {
        let mut events = vec![];
        
        match event {
            PoolEvent::BlockNotarized { slot, block_hash, .. } => {
                debug!("Votor handling block notarized slot={} block_hash={}", slot, format_hash(&block_hash));

                let final_vote_data = self.create_final_vote_data(slot);
                let signature = self.keypair.sign(&final_vote_data);
                
                let state = self.slot_states.entry(slot).or_default();
                state.block_notarized = true;
                
                // voted for this block and it got notarized -> cast final vote
                if let Some(our_vote_hash) = state.cast_notar_vote {
                    if our_vote_hash == block_hash && !state.cast_final_vote {

                        let final_vote = FinalVote {
                            slot,
                            validator_id: self.validator_id.clone(),
                            signature
                        };
                        
                        state.cast_final_vote = true;
                        
                        info!("Votor casting finalization vote slot={} block_hash={}", slot, format_hash(&block_hash));
                        events.push(VotorEvent::ShouldCastVote {
                            vote: AlpenglowMessage::FinalVote(final_vote)
                        });
                    }
                }
            }
            
            PoolEvent::FastFinalized { slot, block_hash, .. } => {
                info!("Votor handling fast finalization slot={} block_hash={}", slot, format_hash(&block_hash));
                
                let state = self.slot_states.entry(slot).or_default();
                state.block_fast_finalized = true;
                
                events.push(VotorEvent::BlockFinalized { slot, block_hash });
            }
            
            PoolEvent::SlotSkipped { slot, .. } => {
                info!("Votor handling slot skipped slot={}", slot);
                
                let state = self.slot_states.entry(slot).or_default();
                state.slot_skipped = true;
                
                events.push(VotorEvent::SlotSkipped { slot });
            }
        }
        
        self.pending_events.extend(events.clone());
        events
    }

    fn create_final_vote_data(&self, slot: Slot) -> Vec<u8> {
        let mut vote_data = Vec::new();
        vote_data.extend_from_slice(&slot.to_le_bytes());
        vote_data.extend_from_slice(b"FINAL");  // Add final marker
        vote_data.extend_from_slice(self.validator_id.as_bytes());
        vote_data
    }
    
    /// decide whether to skip the slot
    pub fn handle_slot_timeout(&mut self, slot: Slot) -> Vec<VotorEvent> {
        debug!("Votor handling slot timeout slot={}", slot);
        
        let mut events = vec![];

        // re-check signture for skip vote?
        let skip_data = self.create_skip_vote_data(slot);
        let signature = self.keypair.sign(&skip_data);


        let state = self.slot_states.entry(slot).or_default();
        
        // haven't voted yet and no block received ? -> vote to skip
        if state.cast_notar_vote.is_none() && !state.cast_skip_vote && state.received_block.is_none() {

            let skip_vote = SkipVote {
                slot,
                validator_id: self.validator_id.clone(),
                signature,
            };
            
            state.cast_skip_vote = true;
            
            info!("Votor casting skip vote due to timeout slot={}", slot);
            events.push(VotorEvent::ShouldCastVote {
                vote: AlpenglowMessage::SkipVote(skip_vote)
            });
        }
        
        self.pending_events.extend(events.clone());
        events
    }
    
    fn create_skip_vote_data(&self, slot: Slot) -> Vec<u8> {
        let mut vote_data = Vec::new();
        vote_data.extend_from_slice(&slot.to_le_bytes());
        vote_data.extend_from_slice(b"SKIP");  // add skip marker
        vote_data.extend_from_slice(self.validator_id.as_bytes());
        vote_data
    }

    /// called when slot begins
    pub fn set_slot_timeout(&mut self, slot: Slot, timeout_duration: Duration) {
        let timeout_time = Instant::now() + timeout_duration;
        self.slot_timeouts.insert(slot, timeout_time);
        
        let state = self.slot_states.entry(slot).or_default();
        state.timeout_set = Some(timeout_time);
        
        debug!("Votor set timeout for slot={} duration={}ms", slot, timeout_duration.as_millis());
    }
    
    /// check for any expired timeouts
    pub fn check_timeouts(&mut self) -> Vec<VotorEvent> {
        let now = Instant::now();
        let mut events = vec![];
        
        let expired_slots: Vec<Slot> = self.slot_timeouts
            .iter()
            .filter(|(_, timeout_time)| now >= **timeout_time)
            .map(|(&slot, _)| slot)
            .collect();
        
        for slot in expired_slots {
            self.slot_timeouts.remove(&slot);
            events.extend(self.handle_slot_timeout(slot));
        }
        
        events
    }
    
    /// get pending events -> for main loop consumption 
    pub fn drain_events(&mut self) -> Vec<VotorEvent> {
        std::mem::take(&mut self.pending_events)
    }
    
}

fn format_hash(hash: &Hash) -> String {
    hex::encode(&hash[..4])
}