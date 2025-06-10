//! rotor - Alpenglow Block Dissemination Protocol
//!
//! rotor implements Alpenglow's optimized block propagation system that solves
//! the leader bandwidth bottleneck problem in blockchain networks.
//!
//! The Problem:
//! blockchains require leaders to send full blocks to every validator:
//! - Leader bandwidth: O(n) where n = number of validators, TowerBFT uses (tree) hops -> about 32 deep tower😭
//! - slot(400ms) * 32 = 12.8s finality
//! - single point of failure for block propagation ?
//!
//! ## rotor's Solution:
//! 1. **distribute shreds to stake-weighted relays** (O(1) per leader)
//! 4. **relays broadcast to all validators** (O(n) total, distributed load)
//!
//! ## Key Innovations vs Turbine:
//! - **single-hop model** (2δ latency) vs multi-layer tree
//! - **one erasure-coded packet per shred** vs separate data/recovery shreds
//! - **stake-proportional relay selection** for security
//! - **compatible with multicast** systems like DoubleZero // lol not doing this here

use std::collections::HashMap;
use crate::types::*;
use crate::network::NetworkManager;
use reed_solomon_erasure::galois_8::ReedSolomon;
use anyhow::Result;
use log::{debug, error, info, trace, warn};
use serde::{Deserialize, Serialize};
use tokio::time::{Duration, Instant};
use crate::types::bls_serde::{deserialize_bls_signature, serialize_bls_signature};
use std::sync::Arc;
use tokio::sync::Mutex;

/// Configuration parameters for Rotor protocol
#[derive(Debug, Clone)]
pub struct RotorConfig {
    pub data_shreds_per_slice: usize,
    pub total_shreds_per_slice: usize,
    pub max_slice_size: usize,
    pub transmission_timeout: Duration,
}

impl Default for RotorConfig {
    fn default() -> Self {
        RotorConfig {
            data_shreds_per_slice: 32,      // γ = 32 (from whitepaper)
            total_shreds_per_slice: 64,     // Γ = 64 (κ = 2 expansion rate)
            max_slice_size: 1024 * 64,      // 64KB per slice
            transmission_timeout: Duration::from_millis(100),
        }
    }
}

/// slice of a block - the unit of erasure coding
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Slice {
    pub slot: Slot,
    pub slice_index: u32,
    pub is_last_slice: bool,
    pub data: Vec<u8>,
    pub merkle_root: Hash,
}

/// Rotor block dissemination engine
pub struct Rotor {
    /// reed-solomon erasure coder
    erasure_coder: ReedSolomon,
    /// network configuration for relay selection
    network_config: NetworkConfig,
    /// rotor-specific configuration
    config: RotorConfig,
    /// network manager for shred transmission
    network: Arc<Mutex<NetworkManager>>,
    /// local validator ID
    validator_id: ValidatorId,
    keypair: crate::types::KeyPair,
    /// track ongoing transmissions
    active_transmissions: HashMap<Slot, TransmissionState>,
}

#[derive(Debug)]
struct TransmissionState {
    slices_sent: u32,
    total_slices: u32,
    start_time: Instant,
}

/// stake-weighted relay selector
pub struct StakeWeightedSelector {
    network_config: NetworkConfig,
}

impl StakeWeightedSelector {
    pub fn new(network_config: NetworkConfig) -> Self {
        StakeWeightedSelector { network_config }
    }
    
    /// select relays proportional to their stake
    /// implements the sampling algorithm from whitepaper Section 3.1
    pub fn select_relays(&self, count: usize) -> Vec<ValidatorId> {
        let mut relays = Vec::new();
        let total_stake = self.network_config.total_stake();
        
        // for simplicity, use weighted random selection
        // TODO: Implement PS-P (Partition Sampling)
        for _ in 0..count {
            let mut random_stake = rand::random::<f64>() * total_stake as f64;
            
            for validator in &self.network_config.validators {
                random_stake -= validator.stake as f64;
                if random_stake <= 0.0 {
                    relays.push(validator.id.clone());
                    break;
                }
            }
        }
        
        relays
    }
}

impl Rotor {
    pub fn new(
        network_config: NetworkConfig,
        network: Arc<Mutex<NetworkManager>>,
        validator_id: ValidatorId,
        keypair: crate::types::KeyPair,
        config: Option<RotorConfig>,
    ) -> Result<Self> {
        let config = config.unwrap_or_default();
        
        let erasure_coder = ReedSolomon::new(
            config.data_shreds_per_slice,
            config.total_shreds_per_slice - config.data_shreds_per_slice,
        )?;
        
        info!("Initialized Rotor validator_id={} data_shreds={} total_shreds={} expansion_rate={:.1}x",
              validator_id,
              config.data_shreds_per_slice,
              config.total_shreds_per_slice,
              config.total_shreds_per_slice as f32 / config.data_shreds_per_slice as f32);
        
        Ok(Rotor {
            erasure_coder,
            network_config,
            config,
            network,
            validator_id,
            keypair,
            active_transmissions: HashMap::new(),
        })
    }
    
    /// entry ->disseminate a block using Rotor protocol
    pub async fn disseminate_block(&mut self, block: Block) -> Result<()> {
        let slot = block.slot;
        let start_time = Instant::now();
        
        info!("Rotor starting block dissemination slot={} size={}KB", 
              slot, block.transactions.len());
        
        // 1: split block into slices for streaming
        let slices = self.split_into_slices(block)?;
        
        // track transmission state
        self.active_transmissions.insert(slot, TransmissionState {
            slices_sent: 0,
            total_slices: slices.len() as u32,
            start_time,
        });
        
        // 2: for each slice, create erasure-coded shreds and disseminate
        for (slice_index, slice) in slices.into_iter().enumerate() {
            debug!("Rotor processing slice slot={} slice_index={} size={}B", 
                   slot, slice_index, slice.data.len());
            
            // create erasure-coded shreds from slice
            let shreds = self.erasure_encode_slice(slice)?;
            
            // select stake-weighted relays for this slice
            let relay_selector = StakeWeightedSelector::new(self.network_config.clone());
            let relays = relay_selector.select_relays(shreds.len());

            let relay_count = relays.len(); 
            
            // send each shred to its designated relay
            self.disseminate_shreds(shreds, relays).await?;
            
            // update transmission progress
            if let Some(state) = self.active_transmissions.get_mut(&slot) {
                state.slices_sent += 1;
            }
            
            debug!("Rotor completed slice slot={} slice_index={} relays={}", 
                   slot, slice_index, relay_count);
        }
        
        let total_time = start_time.elapsed();
        info!("Rotor completed block dissemination slot={} slices={} time={}ms", 
              slot, self.active_transmissions.get(&slot).map(|s| s.total_slices).unwrap_or(0), 
              total_time.as_millis());
        
        self.active_transmissions.remove(&slot);
        
        Ok(())
    }
    
    /// split block into slices for streaming transmission
    /// enables pipelining - can start sending before block is fully built
    fn split_into_slices(&self, block: Block) -> Result<Vec<Slice>> {
        let block_data = bincode::serialize(&block)?;
        let mut slices = Vec::new();
        
        let chunks = block_data.chunks(self.config.max_slice_size);
        let total_chunks = chunks.len();
        
        for (index, chunk) in chunks.enumerate() {
            let slice = Slice {
                slot: block.slot,
                slice_index: index as u32,
                is_last_slice: index == total_chunks - 1,
                data: chunk.to_vec(),
                merkle_root: self.calculate_merkle_root(chunk),
            };
            slices.push(slice);
        }
        
        debug!("Split block slot={} into {} slices, total_size={}KB", 
               block.slot, slices.len(), block_data.len() / 1024);
        
        Ok(slices)
    }
    
    /// apply RS erasure coding to a slice
    /// creates fault-tolerant shreds - can reconstruct from any γ out of Γ shreds
    fn erasure_encode_slice(&self, slice: Slice) -> Result<Vec<Shred>> {
        let slice_data = bincode::serialize(&slice)?;
        
        let padded_size = (slice_data.len() + self.config.data_shreds_per_slice - 1) 
            / self.config.data_shreds_per_slice * self.config.data_shreds_per_slice;
        let mut padded_data = slice_data;
        padded_data.resize(padded_size, 0);
        
        // split into data shreds
        let chunk_size = padded_data.len() / self.config.data_shreds_per_slice;
        let mut data_shreds = Vec::new();
        
        for i in 0..self.config.data_shreds_per_slice {
            let start = i * chunk_size;
            let end = start + chunk_size;
            data_shreds.push(padded_data[start..end].to_vec());
        }
        
        // create parity shreds for fault tolerance
        let parity_count = self.config.total_shreds_per_slice - self.config.data_shreds_per_slice;
        let mut parity_shreds = vec![vec![0u8; chunk_size]; parity_count];
        
        // apply RS encoding
        self.erasure_coder.encode_sep(&mut data_shreds, &mut parity_shreds)?;
        
        // combine data and parity shreds
        let mut all_shred_data = data_shreds;
        all_shred_data.extend(parity_shreds);
        
        // create shred objects with merkle proofs
        let mut shreds = Vec::new();
        let merkle_tree = self.build_merkle_tree(&all_shred_data);
        
        for (index, shred_data) in all_shred_data.into_iter().enumerate() {
            let merkle_proof = self.generate_merkle_proof(&merkle_tree, index);
            let signature = self.sign_shred(&slice, index as u32, &shred_data);
            
            let shred = Shred {
                slot: slice.slot,
                slice_index: slice.slice_index,
                shred_index: index as u32,
                is_last_slice: slice.is_last_slice,
                data: shred_data,
                merkle_root: slice.merkle_root,
                merkle_proof,
                leader_signature: signature,
            };
            
            shreds.push(shred);
        }
        
        debug!("Erasure encoded slice slot={} slice_index={} into {} shreds ({}+{} data+parity)", 
               slice.slot, slice.slice_index, shreds.len(), 
               self.config.data_shreds_per_slice, parity_count);
        
        Ok(shreds)
    }
    
    /// send shreds to their designated relays
    /// each relay will then broadcast their shred to all validators
    async fn disseminate_shreds(&self, shreds: Vec<Shred>, relays: Vec<ValidatorId>) -> Result<()> {
        if shreds.len() != relays.len() {
            return Err(anyhow::anyhow!("Shred count {} != relay count {}", shreds.len(), relays.len()));
        }
        
        // send each shred to its designated relay
        for (shred, relay_id) in shreds.iter().zip(relays.iter()) {
            let shred_msg = AlpenglowMessage::Shred(shred.clone());
            
            match self.network.lock().await.send_to(relay_id, &shred_msg).await {
                Ok(_) => {
                    trace!("Sent shred slot={} slice_index={} shred_index={} to relay={}", 
                           shred.slot, shred.slice_index, shred.shred_index, relay_id);
                }
                Err(e) => {
                    warn!("Failed to send shred to relay={}: {}", relay_id, e);
                }
            }
        }
        
        debug!("Disseminated {} shreds to {} relays", shreds.len(), relays.len());
        Ok(())
    }
    
    // helper methods for merkle tree construction
    fn calculate_merkle_root(&self, data: &[u8]) -> Hash {
        use sha2::{Sha256, Digest};
        let mut hasher = Sha256::new();
        hasher.update(data);
        hasher.finalize().into()
    }
    
    fn build_merkle_tree(&self, shred_data: &[Vec<u8>]) -> Vec<Hash> {
        // simplified Merkle tree
        shred_data.iter().map(|data| self.calculate_merkle_root(data)).collect()
    }
    
    fn generate_merkle_proof(&self, tree: &[Hash], index: usize) -> Vec<Hash> {
        // simplified proof 
        if index < tree.len() {
            vec![tree[index]]
        } else {
            vec![]
        }
    }
    
    fn sign_shred(&self, slice: &Slice, shred_index: u32, shred_data: &[u8]) -> Signature {
        let mut data_to_sign = Vec::new();
        data_to_sign.extend_from_slice(&slice.slot.to_le_bytes());
        data_to_sign.extend_from_slice(&slice.slice_index.to_le_bytes());
        data_to_sign.extend_from_slice(&shred_index.to_le_bytes());
        data_to_sign.extend_from_slice(shred_data);
        
        self.keypair.sign(&data_to_sign)  // bls signing
    }
    
    /// get transmission statistics for monitoring
    pub fn get_transmission_stats(&self, slot: Slot) -> Option<(u32, u32, Duration)> {
        self.active_transmissions.get(&slot).map(|state| {
            (state.slices_sent, state.total_slices, state.start_time.elapsed())
        })
    }
}

/// handle incoming shreds (for relay nodes)
pub struct ShredReceiver {
    received_shreds: HashMap<(Slot, u32), Vec<Option<Shred>>>, // (slot, slice_index) -> shreds
    network: NetworkManager,
    validator_id: ValidatorId,
    config: RotorConfig,
}

impl ShredReceiver {
    pub fn new(network: NetworkManager, validator_id: ValidatorId) -> Self {
        ShredReceiver {
            received_shreds: HashMap::new(),
            network,
            validator_id,
            config: RotorConfig::default(),
        }
    }
    
    pub async fn handle_shred(&mut self, shred: Shred) -> Result<Option<Block>> {
        let key = (shred.slot, shred.slice_index);
        
        // Store the shred
        let shreds = self.received_shreds.entry(key).or_insert_with(|| {
            vec![None; self.config.total_shreds_per_slice]
        });
        
        if shred.shred_index < shreds.len() as u32 {
            shreds[shred.shred_index as usize] = Some(shred.clone());
            
            // Check if we can reconstruct the slice
            let received_count = shreds.iter().filter(|s| s.is_some()).count();
            if received_count >= self.config.data_shreds_per_slice {

                if let Ok(reconstructed_slice) = Self::reconstruct_slice_static(&shreds, self.config.data_shreds_per_slice) {
                    // if this was the last slice, try to reconstruct the full block
                    if reconstructed_slice.is_last_slice {
                        return self.try_reconstruct_block(shred.slot).await;
                    }
                }
            }
        }
        
        // relay the shred to other nodes (if we're a relay)
        self.relay_shred(shred).await?;
        
        Ok(None)
    }

    fn reconstruct_slice_static(shreds: &[Option<Shred>], data_shreds_per_slice: usize) -> Result<Slice> {
        let mut data_chunks = Vec::new();
        let mut indices = Vec::new();
        
        for (i, shred_opt) in shreds.iter().enumerate() {
            if let Some(shred) = shred_opt {
                data_chunks.push(shred.data.clone());
                indices.push(i);
                
                if data_chunks.len() >= data_shreds_per_slice { 
                    break;
                }
            }
        }
        
        // Use Reed-Solomon to reconstruct missing data
        // TODO: Implement proper Reed-Solomon reconstruction
        
        // concatenate available data chunks
        let reconstructed_data: Vec<u8> = data_chunks.into_iter().flatten().collect();
        
        let slice: Slice = bincode::deserialize(&reconstructed_data)?;
        Ok(slice)
    }
    
    fn reconstruct_slice(&self, shreds: &[Option<Shred>]) -> Result<Slice> {
        Self::reconstruct_slice_static(shreds, self.config.data_shreds_per_slice)
    }
    
    async fn try_reconstruct_block(&mut self, slot: Slot) -> Result<Option<Block>> {
        let mut slices = Vec::new();
        let mut slice_index = 0u32;
        
        loop {
            let key = (slot, slice_index);
            if let Some(shreds) = self.received_shreds.get(&key) {
                if let Ok(slice) = self.reconstruct_slice(shreds) {
                    slices.push(slice.clone());
                    if slice.is_last_slice {
                        break;
                    }
                    slice_index += 1;
                } else {
                    return Ok(None); // missing slice data
                }
            } else {
                return Ok(None); // missing slice
            }
        }
        
        let mut block_data = Vec::new();
        for slice in slices {
            block_data.extend(slice.data);
        }
        
        let block: Block = bincode::deserialize(&block_data)?;
        
        info!("Reconstructed block slot={} from {} slices", slot, slice_index + 1);
        Ok(Some(block))
    }
    
    async fn relay_shred(&self, shred: Shred) -> Result<()> {
        // broadcast shred to all other nodes
        let shred_msg = AlpenglowMessage::Shred(shred);
        self.network.broadcast(&shred_msg).await?;
        Ok(())
    }
}