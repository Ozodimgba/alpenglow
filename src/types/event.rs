use crate::types::*;

#[derive(Debug, Clone)]
pub enum PoolEvent {
    BlockNotarized { slot: Slot, certificate: NotarCertificate },
    FastFinalized { slot: Slot, certificate: FastFinalCertificate },
    SlotSkipped { slot: Slot, certificate: SkipCertificate },
}

#[derive(Debug, Clone)]
pub enum VotorEvent {
    ShouldVoteNotar { slot: Slot, block_hash: Hash },
    ShouldSkipSlot { slot: Slot },
    ShouldVoteFinal { slot: Slot },
}