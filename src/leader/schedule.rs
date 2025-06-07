use crate::types::{ValidatorId, Slot};

pub struct LeaderSchedule {
    validators: Vec<ValidatorId>,
}

impl LeaderSchedule {
    pub fn new(mut validators: Vec<ValidatorId>) -> Self {
        validators.sort(); // deterministic ordering
        LeaderSchedule { validators }
    }
    
    pub fn get_leader(&self, slot: Slot) -> &ValidatorId {
        let index = (slot as usize) % self.validators.len();
        &self.validators[index]
    }
    
    pub fn is_leader(&self, slot: Slot, validator_id: &ValidatorId) -> bool {
        self.get_leader(slot) == validator_id
    }
    
    pub fn next_leader_slot(&self, current_slot: Slot, validator_id: &ValidatorId) -> Slot {
        for slot in (current_slot + 1)..=(current_slot + self.validators.len() as u64) {
            if self.is_leader(slot, validator_id) {
                return slot;
            }
        }
        current_slot + self.validators.len() as u64 // fallback
    }
}