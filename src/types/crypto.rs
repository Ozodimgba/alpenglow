use sha2::{Sha256, Digest};
use super::messages::{Hash, Signature, ValidatorId};
use bls_signatures::{PrivateKey, PublicKey, Serialize, Signature as BlsSignature};

#[derive(Debug, Clone)]
pub struct KeyPair {
    pub validator_id: ValidatorId,
    pub private_key: PrivateKey,
    pub public_key: PublicKey,
}

impl KeyPair {
    pub fn new(validator_id: ValidatorId) -> Self {
        let mut hasher = Sha256::new();
        hasher.update(validator_id.as_bytes());
        hasher.update(b"bls_seed");
        let seed: [u8; 32] = hasher.finalize().into();
        
        let private_key = PrivateKey::from_bytes(&seed)
            .expect("Failed to create BLS private key");
        
        let public_key = private_key.public_key();
        
        KeyPair {
            validator_id,
            private_key,
            public_key,
        }
    }
    
    pub fn sign(&self, data: &[u8]) -> BlsSignature {
        self.private_key.sign(data)
    }
    
    pub fn verify(&self, data: &[u8], signature: &BlsSignature) -> bool {
        self.public_key.verify(*signature, data)
    }
}

pub fn hash_data(data: &[u8]) -> Hash {
    let mut hasher = Sha256::new();
    hasher.update(data);
    hasher.finalize().into()
}