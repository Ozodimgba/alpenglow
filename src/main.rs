use std::{env, net::SocketAddr};

use alpenglow_mini::{
    types::{AlpenglowMessage, NetworkConfig, ValidatorId, Slot, ValidatorInfo},
    network::{NetworkManager},
    consensus::Blokstor,
    leader::{LeaderSchedule, BlockBuilder},
};

use tokio::time::{interval, Duration};
use anyhow::Result;
use log::{info, warn, error, debug, trace};

struct AlpenglowNode {
    network: NetworkManager,
    blokstor: Blokstor,
    leader_schedule: LeaderSchedule,
    block_builder: BlockBuilder,
    validator_id: ValidatorId,
    current_slot: Slot,
}

impl AlpenglowNode {
    pub async fn new(config: NetworkConfig) -> Result<Self> {
        let validator_id = config.local_validator_id.clone();
        let network = NetworkManager::new(config.clone()).await?;
        
        let validators: Vec<ValidatorId> = config.validators.iter()
            .map(|v| v.id.clone())
            .collect();

        info!("Initialized AlpenglowNode validator_id={}", validator_id);
        debug!("Network configuration: {} validators, total_stake={}", 
               config.validators.len(), config.total_stake());
        
        Ok(AlpenglowNode {
            network,
            blokstor: Blokstor::new(),
            leader_schedule: LeaderSchedule::new(validators),
            block_builder: BlockBuilder::new(validator_id.clone()),
            validator_id,
            current_slot: 1, // start after genesis
        })
    }
    
    pub async fn run(&mut self) -> Result<()> {
        info!("Starting Alpenglow consensus validator_id={}", self.validator_id);
        
        // send hello to all peers
        if let Err(e) = self.network.broadcast(&AlpenglowMessage::Hello { 
            from: self.validator_id.clone() 
        }).await {
            error!("Failed to broadcast hello message: {}", e);
        }
        
        let mut slot_timer = interval(Duration::from_millis(400));
        info!("Started slot timer with 400ms intervals");
        
        loop {
            tokio::select! {
                Ok((peer_id, msg)) = self.network.receive() => {
                    self.handle_message(peer_id, msg).await?;
                }
                
                _ = slot_timer.tick() => {
                    self.advance_slot().await?;
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
                
                match self.blokstor.store_block(block.clone()) {
                    Ok(_) => {
                        info!("Stored block slot={} hash={} tx_count={}", 
                              block.slot, format_hash(&block.hash()), block.transactions.len());
                    }
                    Err(e) => {
                        warn!("Failed to store block slot={} hash={} error={}", 
                              block.slot, format_hash(&block.hash()), e);
                    }
                }
            }
            AlpenglowMessage::NotarVote(vote) => {
                debug!("Received notarization vote slot={} from peer_id={}", 
                       vote.slot, peer_id);
            }
            AlpenglowMessage::SkipVote(vote) => {
                debug!("Received skip vote slot={} from peer_id={}", 
                       vote.slot, peer_id);
            }
            _ => {
                trace!("Received message type={} from peer_id={}", 
                       message_type(&msg), peer_id);
            }
        }
        Ok(())
    }
    
    async fn advance_slot(&mut self) -> Result<()> {
        info!("Advancing to slot={}", self.current_slot);
        
        // Check if we're the leader
        if self.leader_schedule.is_leader(self.current_slot, &self.validator_id) {
            info!("Starting leader duties slot={} validator_id={}", 
                  self.current_slot, self.validator_id);
            
            match self.block_builder.build_block(self.current_slot, &self.blokstor) {
                Some(block) => {
                    let block_hash = block.hash();
                    info!("Built block slot={} hash={} tx_count={}", 
                          block.slot, format_hash(&block_hash), block.transactions.len());
                    
                    // Store our own block
                    if let Err(e) = self.blokstor.store_block(block.clone()) {
                        error!("Failed to store own block slot={} error={}", block.slot, e);
                        return Ok(());
                    }
                    
                    // Broadcast to peers
                    debug!("Broadcasting block slot={} to network", block.slot);
                    if let Err(e) = self.network.broadcast(&AlpenglowMessage::Block(block)).await {
                        error!("Failed to broadcast block slot={} error={}", self.current_slot, e);
                    } else {
                        info!("Successfully broadcast block slot={}", self.current_slot);
                    }
                }
                None => {
                    warn!("Failed to build block slot={} validator_id={}", 
                          self.current_slot, self.validator_id);
                }
            }
        } else {
            let leader = self.leader_schedule.get_leader(self.current_slot);
            debug!("Waiting for block slot={} leader={}", self.current_slot, leader);
        }
        
        self.current_slot += 1;
        Ok(())
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
            address: "127.0.0.1:8001".parse::<SocketAddr>()?,
            stake: 100,
        },
        ValidatorInfo {
            id: "bob".to_string(),
            address: "127.0.0.1:8002".parse::<SocketAddr>()?,
            stake: 100,
        },
        ValidatorInfo {
            id: "charlie".to_string(),
            address: "127.0.0.1:8003".parse::<SocketAddr>()?,
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
        AlpenglowMessage::NotarCertificate(_) => "NotarCertificate",
        AlpenglowMessage::SkipCertificate(_) => "SkipCertificate",
        AlpenglowMessage::Hello { .. } => "Hello",
        AlpenglowMessage::BlockRequest { .. } => "BlockRequest",
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