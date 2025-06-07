use crate::types::{Block, Transaction, ValidatorId, Slot, Hash};
use crate::consensus::blokstor::Blokstor;

pub struct BlockBuilder {
    validator_id: ValidatorId,
}

impl BlockBuilder {
    pub fn new(validator_id: ValidatorId) -> Self {
        BlockBuilder { validator_id }
    }
    
    pub fn build_block(&self, slot: Slot, blokstor: &Blokstor) -> Option<Block> {
        let parent = blokstor.get_chain_head()?;
        let parent_hash = parent.hash();
        
        // create empty blocks
        let transactions = vec![
            Transaction {
                data: format!("Block {} by {}", slot, self.validator_id).into_bytes(),
                from: self.validator_id.clone(),
            }
        ];
        
        Some(Block {
            slot,
            parent_hash,
            transactions,
            leader_id: self.validator_id.clone(),
            timestamp: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_millis() as u64,
        })
    }
}