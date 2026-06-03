use crate::{Amount, Slot};

pub fn floor_log2(value: Slot) -> u32 {
    if value == 0 {
        0
    } else {
        Slot::BITS - 1 - value.leading_zeros()
    }
}

pub fn log_time_weight(hold_time_slots: Slot, amount: Amount) -> Option<Amount> {
    amount.checked_mul(floor_log2(hold_time_slots) as Amount)
}

pub fn mul_div_floor(a: Amount, b: Amount, denom: Amount) -> Option<Amount> {
    if denom == 0 {
        return None;
    }
    a.checked_mul(b)?.checked_div(denom)
}
