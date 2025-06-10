use std::{env, net::SocketAddr};

use alpenglow_mini::{
    consensus::{self, Blokstor, Pool, Rotor, Votor}, 
    leader::{BlockBuilder, LeaderSchedule}, 
    network::NetworkManager, 
    types::{AlpenglowMessage, Block, ChainSyncRequest, ChainSyncResponse, KeyPair, NetworkConfig, Slot, ValidatorId, ValidatorInfo}
};

use tokio::time::{interval, Duration};
use anyhow::Result;
use log::{info, warn, error, debug, trace};
use std::sync::Arc;
use tokio::sync::Mutex;

struct AlpenglowNode {
    network: Arc<Mutex<NetworkManager>>,
    blokstor: Blokstor,
    pool: Pool,   
    votor: Votor,
    rotor: Rotor,   
    leader_schedule: LeaderSchedule,
    block_builder: BlockBuilder,
    validator_id: ValidatorId,
    current_slot: Slot,
}

impl AlpenglowNode {
    pub async fn new(config: NetworkConfig) -> Result<Self> {
        let validator_id = config.local_validator_id.clone();
        let network = Arc::new(Mutex::new(NetworkManager::new(config.clone()).await?));
        
        let validators: Vec<ValidatorId> = config.validators.iter()
            .map(|v| v.id.clone())
            .collect();

        info!("Initialized AlpenglowNode validator_id={}", validator_id);
        debug!("Network configuration: {} validators, total_stake={}", 
               config.validators.len(), config.total_stake());

        let keypair = KeyPair::new(validator_id.clone());

        let rotor_network = network.clone();
        
        Ok(AlpenglowNode {
            network,
            blokstor: Blokstor::new(),
            pool: Pool::new(config.clone()),
            votor: Votor::new(validator_id.clone(), config.clone()),
            rotor: Rotor::new(config.clone(), rotor_network, validator_id.clone(), keypair, None)?,
            leader_schedule: LeaderSchedule::new(validators),
            block_builder: BlockBuilder::new(validator_id.clone()),
            validator_id,
            current_slot: 1, // start after genesis
        })
    }
    
    pub async fn run(&mut self) -> Result<()> {
        info!("Starting Alpenglow consensus validator_id={}", self.validator_id);

        if self.blokstor.get_highest_slot() == 0 {
            warn!("No chain found - attempting emergency repair");
            self.request_chain_repair(1).await?;
        }

        for attempt in 1..=3 {
        match self.join_network().await {
            Ok(_) => {
                info!("Successfully joined network on attempt {}", attempt);
                break;
            }
            Err(e) => {
                warn!("Failed to join network attempt {}: {}", attempt, e);
                if attempt == 3 {
                    warn!("Starting without network sync - will sync during operation");
                }
                tokio::time::sleep(Duration::from_secs(1)).await;
            }
        }
       }
        
        self.join_network().await?;

        // send hello to all peers
        self.network.lock().await.broadcast(&AlpenglowMessage::Hello { 
            from: self.validator_id.clone() 
        }).await?;
        
        let mut slot_timer = interval(Duration::from_millis(400));
        info!("Started slot timer with 400ms intervals");
        
        loop {
            tokio::select! {
                result = async {
                    let network_guard = self.network.lock().await;
                    network_guard.receive().await
                } => {
                    match result {
                        Ok((peer_id, msg)) => {
                            self.handle_message(peer_id, msg).await?;
                        }
                        Err(e) => {
                            error!("Network receive error: {}", e);
                        }
                    }
                }
                
                _ = slot_timer.tick() => {
                    self.advance_slot().await?;
                }

                _ = tokio::time::sleep(Duration::from_millis(10)) => {
                    let pool_events = self.pool.drain_events();
                    if !pool_events.is_empty() {
                        self.handle_pool_events(pool_events).await?;
                    }
                    
                    let votor_events = self.votor.drain_events();
                    if !votor_events.is_empty() {
                        self.handle_votor_events(votor_events).await?;
                    }
                }
            }
        }
    }
    
    async fn handle_message(&mut self, peer_id: ValidatorId, msg: AlpenglowMessage) -> Result<()> {
        trace!("Received message from peer_id={} type={}", peer_id, message_type(&msg));
        
        match msg {
            AlpenglowMessage::Hello { from } => {
                debug!("Received hello from peer_id={}", from);
            }
            AlpenglowMessage::Block(block) => {
                info!("Received block slot={} hash={} from peer_id={}", 
                    block.slot, format_hash(&block.hash()), peer_id);
                
                let block_hash = block.hash();
                let block_slot = block.slot;
                
                match self.blokstor.store_block(block.clone()) {
                    Ok(_) => {
                        info!("Stored block slot={} hash={} tx_count={}", 
                            block_slot, format_hash(&block_hash), block.transactions.len());
                        
                        // vote on valid blocks
                        let votor_events = self.votor.handle_block_received(block_slot, block_hash);
                        self.handle_votor_events(votor_events).await?;
                    }
                    Err(e) if e.to_string().contains("Parent block not found") => {
                        warn!("Missing parent for block slot={}, requesting chain repair", block_slot);
                        self.request_chain_repair(block_slot).await?;
                        
                        // try to store again after repair attempt
                        if let Ok(_) = self.blokstor.store_block(block.clone()) {
                            let votor_events = self.votor.handle_block_received(block_slot, block_hash);
                            self.handle_votor_events(votor_events).await?;
                        }
                    }
                    Err(e) => {
                        warn!("Failed to store block slot={} hash={} error={}", 
                            block_slot, format_hash(&block_hash), e);
                    }
                }
            }
            AlpenglowMessage::NotarVote(ref vote) => {
                debug!("EXTERNAL VOTE RECEIVED: slot={} validator_id={} from_peer={}", 
                    vote.slot, vote.validator_id, peer_id);
                
                let pool_events = self.pool.add_vote(msg.clone());
                debug!("POOL EVENTS FROM EXTERNAL VOTE: {}", pool_events.len());
                
                if !pool_events.is_empty() {
                    self.handle_pool_events(pool_events).await?;
                }
            }

            AlpenglowMessage::SkipVote(vote) => {
                debug!("Received skip vote slot={} from peer_id={}", 
                       vote.slot, peer_id);
            }

            AlpenglowMessage::NotarVote(_) | AlpenglowMessage::SkipVote(_) | AlpenglowMessage::FinalVote(_) => {
                debug!("Received vote type={} from peer_id={}", message_type(&msg), peer_id);
                
                let pool_events = self.pool.add_vote(msg.clone());
                debug!("Pool generated {} events from vote", pool_events.len());

                if !pool_events.is_empty() {
                    self.handle_pool_events(pool_events).await?;
                }
            }
            AlpenglowMessage::NotarCertificate(_) | AlpenglowMessage::SkipCertificate(_) | AlpenglowMessage::FastFinalCertificate(_) => {
                debug!("Received certificate type={} from peer_id={}", message_type(&msg), peer_id);
                // TODO: Handle certificate validation and forwarding
            }
            // handle chain synchronization requests
            // "joining" - provide chain state to new nodes
            AlpenglowMessage::ChainSyncRequest(request) => {
                self.handle_chain_sync_request(request).await?;
            }       
            // handle chain synchronization responses  
            // these are processed in join_network()
            AlpenglowMessage::ChainSyncResponse(_) => {
                debug!("Received chain sync response (handled in join_network)");
            }

            AlpenglowMessage::Shred(shred) => {
                debug!("Received shred slot={} slice_index={} shred_index={} from peer_id={}", 
                    shred.slot, shred.slice_index, shred.shred_index, peer_id);
                
                // TODO: Add ShredReceiver to handle shred reconstruction
                // just log that we received it
                trace!("Shred handling not yet implemented");
            }
            
            AlpenglowMessage::SliceRequest { slot, slice_index, from } => {
                debug!("Received slice request slot={} slice_index={} from={}", 
                    slot, slice_index, from);
                // TODO: Handle slice repair requests
            }
        
            _ => {
                trace!("Received message type={} from peer_id={}", 
                       message_type(&msg), peer_id);
            }
        }
        Ok(())
    }

    /// emergency chain repair when node is missing parent blocks
    async fn request_chain_repair(&mut self, missing_slot: Slot) -> Result<()> {
        warn!("Requesting emergency chain repair for slot={}", missing_slot);
        
        // if no valid chain at all, reset to genesis
        if self.blokstor.get_highest_slot() == 0 || self.blokstor.get_blocks_range(0, 10).is_empty() {
            warn!("No valid chain found - performing emergency genesis reset");
            self.blokstor.reset_to_genesis();
            self.current_slot = 1;
        }
        
        if let Ok(sync_response) = self.request_chain_sync().await {
            info!("Received chain repair with {} blocks", sync_response.recent_blocks.len());
            

            let mut blocks = sync_response.recent_blocks;
            blocks.sort_by_key(|b| b.slot);
            
            let mut stored_count = 0;
            for block in blocks {
                match self.blokstor.store_block(block.clone()) {
                    Ok(_) => {
                        stored_count += 1;
                        debug!("Chain repair: stored block slot={}", block.slot);
                    }
                    Err(e) if e.to_string().contains("Parent block not found") => {
                        // force store if we're still building the foundation
                        if stored_count < 5 {
                            self.blokstor.force_store_block(block.clone())?;
                            stored_count += 1;
                            warn!("Chain repair: force stored block slot={}", block.slot);
                        }
                    }
                    Err(e) => {
                        debug!("Chain repair: couldn't store block slot={}: {}", block.slot, e);
                    }
                }
            }
            
            info!("Chain repair: successfully stored {} blocks", stored_count);
            
            if sync_response.current_slot > self.current_slot {
                info!("Chain repair: advancing from slot {} to {}", 
                    self.current_slot, sync_response.current_slot);
                self.current_slot = sync_response.current_slot;
            }
        }
        
        Ok(())
    }
    
    async fn advance_slot(&mut self) -> Result<()> {

        let network_slot = self.detect_network_slot();
        
        if network_slot > self.current_slot + 3 {
            warn!("Fast-forwarding from slot {} to {}", self.current_slot, network_slot);
            self.current_slot = network_slot;
        } else {
            self.current_slot += 1;
        }
        
        info!("Advancing to slot={} (network_slot={})", self.current_slot, network_slot);
        
        self.votor.set_slot_timeout(self.current_slot, Duration::from_millis(400));
        
        // only try to be leader if we're synchronized
        if network_slot <= self.current_slot + 2 {
            if self.leader_schedule.is_leader(self.current_slot, &self.validator_id) {
                info!("Starting leader duties slot={} validator_id={}", 
                    self.current_slot, self.validator_id);
                
                if let Some(block) = self.block_builder.build_block(self.current_slot, &self.blokstor) {
                    let block_hash = block.hash();
                    let block_slot = block.slot;
                    
                    info!("Built block slot={} hash={} tx_count={}", 
                        block_slot, format_hash(&block_hash), block.transactions.len());
                    
                    match self.blokstor.store_block(block.clone()) {
                        Ok(_) => {
                            // vote on our own block
                            let votor_events = self.votor.handle_block_received(block_slot, block_hash);
                            self.handle_votor_events(votor_events).await?;
                            
                            // broadcast to peers // now use rotor
                            self.rotor.disseminate_block(block).await?;
                            info!("Successfully broadcast block slot={}", block_slot);
                        }
                        Err(e) => {
                            error!("Failed to store own block slot={} error={}", block_slot, e);
                        }
                    }
                }
            } else {
                let leader = self.leader_schedule.get_leader(self.current_slot);
                debug!("Waiting for block slot={} leader={}", self.current_slot, leader);
            }
        } else {
            debug!("Skipping leader duties - not synchronized (current={} network={})", 
                self.current_slot, network_slot);
        }
        
        let timeout_events = self.votor.check_timeouts();
        self.handle_votor_events(timeout_events).await?;
        
        Ok(())
    }

    /// detect the current network slot by looking at recent blocks
    fn detect_network_slot(&self) -> Slot {
        let mut max_slot = self.current_slot;
        
        for slot in (self.current_slot.saturating_sub(10))..=(self.current_slot + 50) {
            if self.blokstor.get_block(slot).is_some() {
                max_slot = max_slot.max(slot);
            }
        }
        
        max_slot
    }

    async fn handle_pool_events(&mut self, events: Vec<consensus::PoolEvent>) -> Result<()> {
        for event in events {
            match event {
                consensus::PoolEvent::BlockNotarized { slot, block_hash, certificate } => {
                    info!("Block notarized slot={} block_hash={}", slot, format_hash(&block_hash));

                    self.network.lock().await.broadcast(&AlpenglowMessage::NotarCertificate(certificate)).await?;
                }
                consensus::PoolEvent::FastFinalized { slot, block_hash, certificate } => {
                    info!("Block fast-finalized slot={} block_hash={}", slot, format_hash(&block_hash));
                    
                    self.network.lock().await.broadcast(&AlpenglowMessage::FastFinalCertificate(certificate)).await?;
                }
                consensus::PoolEvent::SlotSkipped { slot, certificate } => {
                    info!("Slot skipped slot={}", slot);
                    
                    self.network.lock().await.broadcast(&AlpenglowMessage::SkipCertificate(certificate)).await?;
                }
            }
        }
        Ok(())
    }

    async fn handle_votor_events(&mut self, events: Vec<consensus::VotorEvent>) -> Result<()> {
        for event in events {
            match event {
                consensus::VotorEvent::ShouldCastVote { vote } => {
                    info!("Votor decided to cast vote type={}", message_type(&vote));
                    
                    self.network.lock().await.broadcast(&vote).await?;
                    debug!("Broadcast vote type={} to network", message_type(&vote));
                    
                    // add our own vote to the pool
                    let pool_events = self.pool.add_vote(vote);

                    if !pool_events.is_empty() {
                        self.handle_pool_events(pool_events).await?;
                    }
                }
                consensus::VotorEvent::BlockFinalized { slot, block_hash } => {
                    info!("Block finalized slot={} block_hash={}", slot, format_hash(&block_hash));
                    // TODO: nandle block finalization -> update state, notify applications...
                }
                consensus::VotorEvent::SlotSkipped { slot } => {
                    info!("Slot skipped slot={}", slot);
                    // TODO: handle slot skip -> cleanup, move to next slot...
                }
            }
        }
        Ok(())
    }

    /// join an existing Alpenglow network or start a new one
    /// 
    /// alpenglow Section 3.4: "joining"
    /// - find latest finalized block from any peer
    /// - sync from that point forward  
    /// - join consensus at current network slot
    async fn join_network(&mut self) -> Result<()> {
        info!("Attempting to join Alpenglow network validator_id={}", self.validator_id);
        
        // discover existing network
        // whitepaper: "observe a finalization of block b in slot s"
        match self.request_chain_sync().await {
            Ok(sync_info) => {
                info!("Found existing network at slot={}, syncing from finalized_slot={}", 
                      sync_info.current_slot, sync_info.latest_finalized_slot);
                
                // apply sync per section 3.4 joining procedure
                self.apply_chain_sync(sync_info).await?;
            }
            Err(_) => {
                info!("No existing network found, starting from genesis");
                // Start new network from slot 1 (after genesis block 0)
                self.current_slot = 1;
            }
        }
        
        Ok(())
    }

    /// req chain synchronization from network peers
    /// node needs to "observe a finalization certificate for a block b in slot s"
    async fn request_chain_sync(&mut self) -> Result<ChainSyncResponse> {
        debug!("Broadcasting chain sync request to discover network state");
        
        // broadcast request to all known peers
        self.network.lock().await.broadcast(&AlpenglowMessage::ChainSyncRequest(ChainSyncRequest {
            from_validator: self.validator_id.clone(),
        })).await?;
        
        // timeout prevents infinite waiting if network is empty
        let timeout = tokio::time::timeout(Duration::from_secs(3), async {
            loop {
                if let Ok((peer_id, msg)) = self.network.lock().await.receive().await {
                    if let AlpenglowMessage::ChainSyncResponse(response) = msg {
                        debug!("Received chain sync from peer_id={} latest_finalized={} current={}", 
                               peer_id, response.latest_finalized_slot, response.current_slot);
                        return Ok(response);
                    }
                }
                // small delay to prevent busy loop
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        }).await;
        
        timeout.map_err(|_| anyhow::anyhow!("Chain sync discovery timeout - no existing network found"))?
    }


    /// apply received chain synchronization data
    /// 
    /// "block b can be retrieved with Repair Section 2.8"
    /// "retrieve any missing blocks for all slots in parallel"
    async fn apply_chain_sync(&mut self, sync_info: ChainSyncResponse) -> Result<()> {
        info!("Applying chain sync: finalized_slot={} current_slot={} blocks_received={}", 
              sync_info.latest_finalized_slot, sync_info.current_slot, sync_info.recent_blocks.len());
        
        // store all received blocks
        // this implements the parallel block retrieval mentioned in Section 3.4
        let mut stored_count = 0;
        for block in sync_info.recent_blocks {
            match self.blokstor.store_block(block.clone()) {
                Ok(_) => {
                    stored_count += 1;
                    debug!("Synced block slot={} hash={}", block.slot, format_hash(&block.hash()));
                }
                Err(e) => {
                    // Non-fatal: might be duplicate or invalid block
                    debug!("Failed to store sync block slot={}: {}", block.slot, e);
                }
            }
        }
        
        // set our current slot to match the network
        // join consensus at current network state
        self.current_slot = sync_info.current_slot;
        
        info!("Chain sync complete: stored {} blocks, joining consensus at slot={}", 
              stored_count, self.current_slot);
        Ok(())
    }
    
    /// handle incoming chain sync requests from other nodes
    /// 
    /// provide finalization certificates and recent blocks to joining nodes
    async fn handle_chain_sync_request(&mut self, request: ChainSyncRequest) -> Result<()> {
        info!("Received chain sync request from validator_id={}", request.from_validator);
        
        // gather our current chain state for the requesting node
        let recent_blocks = self.get_recent_blocks_for_sync(20);
        let latest_finalized = self.get_latest_finalized_slot();
        
        let response = ChainSyncResponse {
            latest_finalized_slot: latest_finalized,
            latest_finalized_hash: self.get_latest_finalized_hash(),
            current_slot: self.current_slot,
            recent_blocks,
        };
        
        debug!("Sending chain sync response: finalized_slot={} current_slot={} blocks={}", 
               latest_finalized, self.current_slot, response.recent_blocks.len());
        
        // send directly to requesting node
        self.network.lock().await.send_to(&request.from_validator, 
                            &AlpenglowMessage::ChainSyncResponse(response)).await?;
        
        Ok(())
    }

    /// get recent blocks for chain synchronization
    /// 
    /// impls get block range from blkstor ->"retrieve any missing blocks for all slots in parallel"
    fn get_recent_blocks_for_sync(&self, count: usize) -> Vec<Block> {
        let start_slot = self.current_slot.saturating_sub(count as u64);
        self.blokstor.get_blocks_range(start_slot, self.current_slot.saturating_sub(1))
    }
    
    /// get the latest slot that has achieved deterministic finality
    /// 
    /// alpenglow Definition 14: Block finalization
    /// "deterministic finality requires a block to achieve maximum lockout in the vote tower"
    /// 
    /// for simplicity, we consider blocks finalized after sufficient confirmations
    /// TODO: Implement proper finality tracking based on certificates
    fn get_latest_finalized_slot(&self) -> Slot {
        // conservative estimate: blocks 10+ slots old are considered finalized
        // in production, this should track actual finalization certificates but mehhh
        self.current_slot.saturating_sub(10)
    }
    
    /// get hash of the latest finalized block
    /// 
    /// used to verify chain integrity during sync
    fn get_latest_finalized_hash(&self) -> alpenglow_mini::types::Hash {
        let finalized_slot = self.get_latest_finalized_slot();
        
        if let Some(block) = self.blokstor.get_block(finalized_slot) {
            block.hash()
        } else {
            // fallback to genesis hash if no finalized block found
            warn!("No finalized block found at slot={}, using genesis", finalized_slot);
            [0u8; 32] // genesis
        }
    }
}

#[tokio::main]
async fn main() -> Result<()> {

    init_logging();
    
    let args: Vec<String> = env::args().collect();
    
    if args.len() < 2 {
        error!("Missing validator_id argument");
        eprintln!("Usage: {} <validator_id>", args[0]);
        eprintln!("Available validators: alice, bob, charlie");
        std::process::exit(1);
    }
    
    let validator_id = args[1].clone();
    
    info!("Loading configuration validator_id={}", validator_id);
    let config = match load_config(&validator_id) {
        Ok(config) => config,
        Err(e) => {
            error!("Failed to load configuration validator_id={} error={}", validator_id, e);
            std::process::exit(1);
        }
    };
    
    info!("Configuration loaded validator_id={} bind_address={} network_size={}", 
          validator_id, config.bind_address, config.validators.len());
    
    let mut node = match AlpenglowNode::new(config).await {
        Ok(node) => node,
        Err(e) => {
            error!("Failed to initialize node validator_id={} error={}", validator_id, e);
            std::process::exit(1);
        }
    };
    
    info!("Node initialization complete, starting consensus validator_id={}", validator_id);
    
    if let Err(e) = node.run().await {
        error!("Node encountered fatal error validator_id={} error={}", validator_id, e);
        std::process::exit(1);
    }
    
    Ok(())
}

fn init_logging() {
    if env::var("RUST_LOG").is_err() {
        unsafe { env::set_var("RUST_LOG", "info") };
    }

    env_logger::Builder::from_default_env()
        .format_timestamp_millis()
        .format_module_path(false)
        .format_target(false)
        .init();
    
    info!("Logging initialized");
}


fn load_config(validator_id: &str) -> Result<NetworkConfig> {
    // fefine all validators in the network
    let validators = vec![
        ValidatorInfo {
            id: "alice".to_string(),
            address: "127.0.0.1:9001".parse::<SocketAddr>()?,
            stake: 100,
        },
        ValidatorInfo {
            id: "bob".to_string(),
            address: "127.0.0.1:9002".parse::<SocketAddr>()?,
            stake: 100,
        },
        ValidatorInfo {
            id: "charlie".to_string(),
            address: "127.0.0.1:9003".parse::<SocketAddr>()?,
            stake: 100,
        },
    ];
    
    let bind_address = validators.iter()
        .find(|v| v.id == validator_id)
        .map(|v| v.address) 
        .ok_or_else(|| anyhow::anyhow!("Validator '{}' not found in network config", validator_id))?;
    
    let config = NetworkConfig {
        validators,
        local_validator_id: validator_id.to_string(),
        bind_address,  
    };
    
    if config.validators.is_empty() {
        return Err(anyhow::anyhow!("No validators configured"));
    }
    
    if config.total_stake() == 0 {
        return Err(anyhow::anyhow!("Total stake is zero"));
    }
    
    Ok(config)
}

fn message_type(msg: &AlpenglowMessage) -> &'static str {
    match msg {
        AlpenglowMessage::Block(_) => "Block",
        AlpenglowMessage::NotarVote(_) => "NotarVote",
        AlpenglowMessage::SkipVote(_) => "SkipVote",
        AlpenglowMessage::FinalVote(_) => "FinalVote",
        AlpenglowMessage::NotarCertificate(_) => "NotarCertificate",
        AlpenglowMessage::SkipCertificate(_) => "SkipCertificate",
        AlpenglowMessage::FastFinalCertificate(_) => "FastFinalCertificate",
        AlpenglowMessage::Hello { .. } => "Hello",
        AlpenglowMessage::BlockRequest { .. } => "BlockRequest",
        AlpenglowMessage::ChainSyncRequest(_) => "ChainSyncRequest",
        AlpenglowMessage::ChainSyncResponse(_) => "ChainSyncResponse",
        AlpenglowMessage::Shred(_) => "Shred",
        AlpenglowMessage::SliceRequest { .. } => "SliceRequest",
    }
}

fn format_hash(hash: &[u8; 32]) -> String {
    hex::encode(&hash[..8]) 
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_load_config_alice() {
        let config = load_config("alice").unwrap();
        assert_eq!(config.local_validator_id, "alice");
        assert_eq!(config.bind_address.to_string(), "127.0.0.1:8001");
        assert_eq!(config.validators.len(), 3);
        assert_eq!(config.total_stake(), 300);
    }
    
    #[test]
    fn test_load_config_invalid() {
        let result = load_config("invalid");
        assert!(result.is_err());
    }
    
    #[test]
    fn test_load_config_bob() {
        let config = load_config("bob").unwrap();
        assert_eq!(config.local_validator_id, "bob");
        assert_eq!(config.bind_address.to_string(), "127.0.0.1:8002");
    }
}