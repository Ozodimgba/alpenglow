use std::collections::HashMap;
use crate::types::{Block, Slot, Hash};
use anyhow::Result;

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
        if block.slot <= self.latest_slot && block.slot != 0 {
            return Err(anyhow::anyhow!("Block slot {} too old", block.slot));
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
}