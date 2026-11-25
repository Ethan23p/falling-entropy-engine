//! Randomness: counter-based bits from a cell's position, its phase and a salt, and nothing else,
//! so every cell computes the same thing in any order.

/// One round of the lowbias32 integer hash (Chris Wellons).
fn mix(mut v: u32) -> u32 {
    v ^= v >> 16;
    v = v.wrapping_mul(0x7feb_352d);
    v ^= v >> 15;
    v = v.wrapping_mul(0x846c_a68b);
    v ^ (v >> 16)
}

/// Counter-based random bits from a cell's position, its phase and a salt.
pub fn hash(x: i32, y: i32, phase: u32, salt: u32) -> u32 {
    mix((x as u32) ^ mix((y as u32) ^ mix(phase ^ mix(salt))))
}
