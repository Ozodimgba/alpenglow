use serde::{Deserialize, Serialize};
use std::collections::HashMap;

// basic primitive types
pub type ValidatorId = String; 
pub type Hash = [u8; 32];   
pub type Slot = u64; 
pub type Signature = [u8; 64];


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
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkipVote {
    pub slot: Slot,
    pub validator_id: ValidatorId,
}

// certificates simplified - no bls aggregation 
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NotarCertificate {
    pub slot: Slot,
    pub block_hash: Hash,
    pub votes: Vec<NotarVote>,
    pub total_stake: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkipCertificate {
    pub slot: Slot,
    pub votes: Vec<SkipVote>,
    pub total_stake: u64,
}

// All possible message types
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
    
    #[test]
    fn test_voting_messages() {
        let vote = NotarVote {
            slot: 1,
            block_hash: [1u8; 32],
            validator_id: "alice".to_string(),
        };
        
        let msg = AlpenglowMessage::NotarVote(vote);
        let serialized = bincode::serialize(&msg).unwrap();
        let deserialized: AlpenglowMessage = bincode::deserialize(&serialized).unwrap();
        
        match deserialized {
            AlpenglowMessage::NotarVote(v) => {
                assert_eq!(v.slot, 1);
                assert_eq!(v.validator_id, "alice");
            },
            _ => panic!("Wrong message type"),
        }
    }
}