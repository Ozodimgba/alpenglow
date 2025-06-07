use tokio::net::UdpSocket;
use std::net::SocketAddr;
use std::collections::HashMap;
use anyhow::Result;
use crate::types::{AlpenglowMessage, ValidatorId, NetworkConfig};

pub struct NetworkManager {
    socket: UdpSocket,
    peers: HashMap<ValidatorId, SocketAddr>,
    local_id: ValidatorId,
}

impl NetworkManager {
    pub async fn new(config: NetworkConfig) -> Result<Self> {
        let socket = UdpSocket::bind(config.bind_address).await?;
        
        let mut peers = HashMap::new();
        for validator in &config.validators {
            if validator.id != config.local_validator_id {
                peers.insert(validator.id.clone(), validator.address);
            }
        }
        
        Ok(NetworkManager {
            socket,
            peers,
            local_id: config.local_validator_id,
        })
    }
    
    pub async fn send_to(&self, peer_id: &ValidatorId, msg: &AlpenglowMessage) -> Result<()> {
        if let Some(addr) = self.peers.get(peer_id) {
            let data = bincode::serialize(msg)?;
            self.socket.send_to(&data, addr).await?;
        }
        Ok(())
    }
    
    pub async fn broadcast(&self, msg: &AlpenglowMessage) -> Result<()> {
        let data = bincode::serialize(msg)?;
        for addr in self.peers.values() {
            self.socket.send_to(&data, addr).await?;
        }
        Ok(())
    }
    
    pub async fn receive(&self) -> Result<(ValidatorId, AlpenglowMessage)> {
        let mut buf = [0u8; 8192];
        let (len, addr) = self.socket.recv_from(&mut buf).await?;
        
        let msg: AlpenglowMessage = bincode::deserialize(&buf[..len])?;
        
        // find which peer sent this (reverse lookup)
        let peer_id = self.peers
            .iter()
            .find(|(_, peer_addr)| *peer_addr == &addr)
            .map(|(id, _)| id.clone())
            .unwrap_or_else(|| "unknown".to_string());
            
        Ok((peer_id, msg))
    }
}