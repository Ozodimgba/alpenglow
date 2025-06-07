use sha2::{Sha256, Digest};
use super::messages::{Hash, Signature, ValidatorId};

#[derive(Debug, Clone)]
pub struct KeyPair {
    pub validator_id: ValidatorId,
    pub private_key: [u8; 32],
    pub public_key: [u8; 32],
}

impl KeyPair {
    pub fn new(validator_id: ValidatorId) -> Self {
        let mut hasher = Sha256::new();
        hasher.update(validator_id.as_bytes());
        hasher.update(b"private");
        let private_key = hasher.finalize().into();
        
        let mut hasher = Sha256::new();
        hasher.update(validator_id.as_bytes());
        hasher.update(b"public");
        let public_key = hasher.finalize().into();
        
        KeyPair {
            validator_id,
            private_key,
            public_key,
        }
    }
    
    // placeholder signature: just hash the data
    pub fn sign(&self, data: &[u8]) -> Signature {
        let mut hasher = Sha256::new();
        hasher.update(&self.private_key);
        hasher.update(data);
        let hash = hasher.finalize();
        
        let mut sig = [0u8; 64];
        sig[..32].copy_from_slice(&hash);
        sig[32..].copy_from_slice(&self.validator_id.as_bytes()[..32.min(self.validator_id.len())]);
        sig
    }
    
    pub fn verify(&self, data: &[u8], signature: &Signature) -> bool {
        let expected = self.sign(data);
        expected == *signature
    }
}

pub fn hash_data(data: &[u8]) -> Hash {
    let mut hasher = Sha256::new();
    hasher.update(data);
    hasher.finalize().into()
}