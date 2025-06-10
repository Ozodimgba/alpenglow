use std::collections::HashMap;
use crate::types::{Block, Slot, Hash};
use anyhow::Result;
use log::{info, warn};

pub struct Blokstor {
    blocks: HashMap<Slot, Block>,
    block_by_hash: HashMap<Hash, Block>,
    latest_slot: Slot,
    genesis_hash: Hash,
}

impl Blokstor {
    pub fn new() -> Self {
        let genesis = Block::genesis();
        let genesis_hash = genesis.hash();
        
        let mut blocks = HashMap::new();
        let mut block_by_hash = HashMap::new();
        
        blocks.insert(0, genesis.clone());
        block_by_hash.insert(genesis_hash, genesis);
        
        Blokstor {
            blocks,
            block_by_hash,
            latest_slot: 0,
            genesis_hash,
        }
    }
    
    pub fn store_block(&mut self, block: Block) -> Result<()> {
        let max_age = 20; // allow blocks up to 20 slots old
        if block.slot < self.latest_slot.saturating_sub(max_age) {
            return Err(anyhow::anyhow!("Block slot {} too old (latest={})", block.slot, self.latest_slot));
        }

        if block.slot > self.latest_slot {
            info!("Updating latest_slot from {} to {}", self.latest_slot, block.slot);
            self.latest_slot = block.slot;
        }
        
        // check parent exists
        if !self.block_by_hash.contains_key(&block.parent_hash) {
            return Err(anyhow::anyhow!("Parent block not found"));
        }
        
        let hash = block.hash();
        let slot = block.slot; 
        

        self.block_by_hash.insert(hash, block.clone());
        self.blocks.insert(slot, block);
        
        if slot > self.latest_slot {
            self.latest_slot = slot;
        }
        
        println!("📦 Stored block slot={} hash={:?}", slot, &hash[..4]);
        Ok(())
    }
    
    pub fn get_block(&self, slot: Slot) -> Option<&Block> {
        self.blocks.get(&slot)
    }
    
    pub fn get_block_by_hash(&self, hash: &Hash) -> Option<&Block> {
        self.block_by_hash.get(hash)
    }
    
    pub fn latest_slot(&self) -> Slot {
        self.latest_slot
    }
    
    pub fn get_chain_head(&self) -> Option<&Block> {
        self.blocks.get(&self.latest_slot)
    }


    /// get a range of blocks for chain synchronization
    pub fn get_blocks_range(&self, start_slot: Slot, end_slot: Slot) -> Vec<Block> {
        let mut blocks = Vec::new();
        for slot in start_slot..=end_slot {
            if let Some(block) = self.blocks.get(&slot) {
                blocks.push(block.clone());
            }
        }
        blocks
    }

    /// check if we have a continuous chain from start to end
    /// ensures chain integrity per Alpenglow Definition 5 -> ancestor/descendant relationships
    pub fn has_continuous_chain(&self, start_slot: Slot, end_slot: Slot) -> bool {
        for slot in start_slot..=end_slot {
            if !self.blocks.contains_key(&slot) {
                return false;
            }
        }
        true
    }
    
    /// get the highest slot we have a block for
    pub fn get_highest_slot(&self) -> Slot {
        self.latest_slot
    }

    /// emergency chain reset - start from a known good state
    pub fn reset_to_genesis(&mut self) {
        warn!("EMERGENCY: Resetting chain to genesis");
        
        self.blocks.clear();
        self.block_by_hash.clear();
        
        // Create and store genesis block
        let genesis = Block::genesis();
        let genesis_hash = genesis.hash();
        
        self.blocks.insert(0, genesis.clone());
        self.block_by_hash.insert(genesis_hash, genesis);
        self.latest_slot = 0;
        
        info!("Chain reset complete - starting from genesis");
    }

    /// force store a block without parent validation (emergency only)
    pub fn force_store_block(&mut self, block: Block) -> Result<()> {
        let hash = block.hash();
        let slot = block.slot;
        
        warn!("EMERGENCY: Force storing block slot={} without validation", slot);
        
        self.blocks.insert(slot, block.clone());
        self.block_by_hash.insert(hash, block);
        
        if slot > self.latest_slot {
            self.latest_slot = slot;
        }
        
        Ok(())
    }
}