use serde::{Deserialize, Serialize};
use std::net::SocketAddr;
use super::messages::ValidatorId;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ValidatorInfo {
    pub id: ValidatorId,
    pub address: SocketAddr,
    pub stake: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NetworkConfig {
    pub validators: Vec<ValidatorInfo>,
    pub local_validator_id: ValidatorId,
    pub bind_address: SocketAddr,
}

impl NetworkConfig {
    pub fn get_validator(&self, id: &ValidatorId) -> Option<&ValidatorInfo> {
        self.validators.iter().find(|v| &v.id == id)
    }
    
    pub fn total_stake(&self) -> u64 {
        self.validators.iter().map(|v| v.stake).sum()
    }
    
    pub fn get_stake(&self, id: &ValidatorId) -> u64 {
        self.get_validator(id).map(|v| v.stake).unwrap_or(0)
    }
}