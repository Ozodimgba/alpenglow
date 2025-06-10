use bls_signatures::{Serialize, Signature as BlsSignature};
use serde::{Deserialize, Deserializer, Serialize as SerdeSerilizer, Serializer};

pub fn serialize_bls_signature<S>(signature: &BlsSignature, serializer: S) -> Result<S::Ok, S::Error>
where
    S: Serializer,
{
    let bytes = signature.as_bytes();
    serializer.serialize_bytes(&bytes)
}

pub fn deserialize_bls_signature<'de, D>(deserializer: D) -> Result<BlsSignature, D::Error>
where
    D: Deserializer<'de>,
{
    let bytes: Vec<u8> = Vec::deserialize(deserializer)?;
    BlsSignature::from_bytes(&bytes)
        .map_err(|e| serde::de::Error::custom(format!("Invalid BLS signature: {}", e)))
}

pub fn serialize_bls_signatures<S>(signatures: &[BlsSignature], serializer: S) -> Result<S::Ok, S::Error>
where
    S: Serializer,
{
    let bytes_vec: Vec<Vec<u8>> = signatures.iter()
        .map(|sig| sig.as_bytes().to_vec())
        .collect();
    bytes_vec.serialize(serializer)
}

pub fn deserialize_bls_signatures<'de, D>(deserializer: D) -> Result<Vec<BlsSignature>, D::Error>
where
    D: Deserializer<'de>,
{
    let bytes_vec: Vec<Vec<u8>> = Vec::deserialize(deserializer)?;
    bytes_vec.into_iter()
        .map(|bytes| BlsSignature::from_bytes(&bytes)
            .map_err(|e| serde::de::Error::custom(format!("Invalid BLS signature: {}", e))))
        .collect()
}