//! Selection: choosing among viable options. By default it considers them all together and picks
//! at random; evaluate and propose bring the physics distribution, which excludes some options
//! (weight zero) and biases the weights of the rest.

/// Weighted selection: the option whose running total of weights first passes `bits` mod the
/// total. Returns its index, or `None` when every weight is zero.
pub fn select(weights: &[u32; 6], bits: u32) -> Option<usize> {
    let total: u32 = weights.iter().sum();
    if total == 0 {
        return None;
    }
    let mut r = bits % total;
    for (i, &w) in weights.iter().enumerate() {
        if r < w {
            return Some(i);
        }
        r -= w;
    }
    None
}
