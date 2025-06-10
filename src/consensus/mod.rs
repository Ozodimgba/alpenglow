//! ## rotor vs Votor:
//! - **rotor**: gets data around the network -> dissemination
//! - **votor**: decides what to do with received data -> consensus
pub mod blokstor;
pub mod pool;
pub mod votor;
pub mod rotor;

pub use blokstor::*;
pub use pool::*;
pub use votor::*;
pub use rotor::*;