use serde::{Deserialize, Serialize};
use bls_signatures::Signature as BlsSignature;
use crate::types::bls_serde::{serialize_bls_signature, deserialize_bls_signature}; // should be a macro iono, PR?

// basic primitive types
pub type ValidatorId = String; 
pub type Hash = [u8; 32];   
pub type Slot = u64; 
pub type Signature = BlsSignature;


#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Block {
    pub slot: Slot,
    pub parent_hash: Hash,
    pub transactions: Vec<Transaction>,
    pub leader_id: ValidatorId,
    pub timestamp: u64,
}

// Simple transaction 
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Transaction {
    pub data: Vec<u8>,
    pub from: ValidatorId,
}

// Voting messages
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NotarVote {
    pub slot: Slot,
    pub block_hash: Hash,
    pub validator_id: ValidatorId,
    #[serde(serialize_with = "serialize_bls_signature", deserialize_with = "deserialize_bls_signature")]
    pub signature: Signature,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkipVote {
    pub slot: Slot,
    pub validator_id: ValidatorId,
    #[serde(serialize_with = "serialize_bls_signature", deserialize_with = "deserialize_bls_signature")]
    pub signature: Signature,
}

// certificates simplified - no bls aggregation 
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NotarCertificate {
    pub slot: Slot,
    pub block_hash: Hash,
    #[serde(serialize_with = "serialize_bls_signature", deserialize_with = "deserialize_bls_signature")]
    pub aggregated_signature: BlsSignature,
    pub signing_validators: Vec<ValidatorId>,
    pub total_stake: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkipCertificate {
    pub slot: Slot,
    pub votes: Vec<SkipVote>,
    pub total_stake: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FastFinalCertificate {
    pub slot: Slot,
    pub block_hash: Hash,
    #[serde(serialize_with = "serialize_bls_signature", deserialize_with = "deserialize_bls_signature")]
    pub aggregated_signature: Signature,
    pub signing_validators: Vec<ValidatorId>,
    pub total_stake: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]  
pub struct FinalVote {
    pub slot: Slot,
    pub validator_id: ValidatorId,
    #[serde(serialize_with = "serialize_bls_signature", deserialize_with = "deserialize_bls_signature")]
    pub signature: Signature, 
}

/// chain Synchronization Messages
/// 
/// based on whitepaper "Asynchrony in Practice" - joining
/// 
/// when a node goes offline or newly joins, it needs to sync with the current
/// network state before participating in consensus.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChainSyncRequest {
    /// validator requesting chain sync
    pub from_validator: ValidatorId,
}

#[derive(Debug, Clone, Serialize, Deserialize)]  
pub struct ChainSyncResponse {
    /// latest block that has achieved deterministic finality
    /// alpenglow Definition 14: Block Finalization
    pub latest_finalized_slot: Slot,
    pub latest_finalized_hash: Hash,
    
    pub current_slot: Slot,
    
    /// recent blocks for fast catch-up
    /// "retrieve any missing blocks for all slots in parallel"
    pub recent_blocks: Vec<Block>,
}


/// Rotor shred - erasure-coded piece of a slice
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Shred {
    pub slot: Slot,
    pub slice_index: u32,
    pub shred_index: u32,
    pub is_last_slice: bool,
    pub data: Vec<u8>,
    pub merkle_root: Hash,
    pub merkle_proof: Vec<Hash>,
     #[serde(serialize_with = "serialize_bls_signature", deserialize_with = "deserialize_bls_signature")]
    pub leader_signature: Signature,
}

/// Block slice - intermediate step between Block and Shreds
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Slice {
    pub slot: Slot,
    pub slice_index: u32,
    pub is_last_slice: bool,
    pub data: Vec<u8>,
    pub merkle_root: Hash,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum AlpenglowMessage {
    // Block dissemination
    Block(Block),
    
    // Voting
    NotarVote(NotarVote),
    SkipVote(SkipVote),
    
    // Certificates
    NotarCertificate(NotarCertificate),
    SkipCertificate(SkipCertificate),
    
    // Network coordination
    Hello { from: ValidatorId },
    
    // Block requests (for repair later)
    BlockRequest { slot: Slot, from: ValidatorId },

    FinalVote(FinalVote),                       
    FastFinalCertificate(FastFinalCertificate),

    ChainSyncRequest(ChainSyncRequest),
    ChainSyncResponse(ChainSyncResponse),

    Shred(Shred),   
    SliceRequest { slot: Slot, slice_index: u32, from: ValidatorId },
}

impl Block {
    pub fn hash(&self) -> Hash {
        use sha2::{Sha256, Digest};
        let serialized = bincode::serialize(self).unwrap();
        let mut hasher = Sha256::new();
        hasher.update(&serialized);
        hasher.finalize().into()
    }
    
    pub fn is_valid_parent(&self, parent: &Block) -> bool {
        self.parent_hash == parent.hash() && self.slot == parent.slot + 1
    }
}

impl Block {
    pub fn genesis() -> Self {
        Block {
            slot: 0,
            parent_hash: [0u8; 32], // genesis
            transactions: vec![],
            leader_id: "genesis".to_string(),
            timestamp: 0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_block_creation_and_hashing() {
        let genesis = Block::genesis();
        let genesis_hash = genesis.hash();
        
        let block1 = Block {
            slot: 1,
            parent_hash: genesis_hash,
            transactions: vec![],
            leader_id: "alice".to_string(),
            timestamp: 1000,
        };
        
        assert!(block1.is_valid_parent(&genesis));
        assert_ne!(genesis_hash, block1.hash());
    }
    
    #[test]
    fn test_message_serialization() {
        let block = Block::genesis();
        let msg = AlpenglowMessage::Block(block);
        
        let serialized = bincode::serialize(&msg).unwrap();
        let deserialized: AlpenglowMessage = bincode::deserialize(&serialized).unwrap();
        
        match deserialized {
            AlpenglowMessage::Block(b) => assert_eq!(b.slot, 0),
            _ => panic!("Wrong message type"),
        }
    }
    
    // #[test]
    // fn test_voting_messages() {
    //     let vote = NotarVote {
    //         slot: 1,
    //         block_hash: [1u8; 32],
    //         validator_id: "alice".to_string(),
    //     };
        
    //     let msg = AlpenglowMessage::NotarVote(vote);
    //     let serialized = bincode::serialize(&msg).unwrap();
    //     let deserialized: AlpenglowMessage = bincode::deserialize(&serialized).unwrap();
        
    //     match deserialized {
    //         AlpenglowMessage::NotarVote(v) => {
    //             assert_eq!(v.slot, 1);
    //             assert_eq!(v.validator_id, "alice");
    //         },
    //         _ => panic!("Wrong message type"),
    //     }
    // }
}