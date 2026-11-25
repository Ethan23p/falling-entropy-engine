//! The physics distribution: the overlay on selection that evaluate and propose apply. It
//! excludes the options whose conditions the particles' materials can't meet, and biases the
//! weights of the rest from the hosted particle's velocity, its mass, and what the other cell holds.
//!
//! The policies so far:
//! - **Movement**: a cell hosting a particle proposes to move it toward a neighbor, weighted by
//!   how well that direction lines up with the particle's velocity: its intent to move. Velocity
//!   is basically a direction for now, at a steady scale (`Velocity::SCALE`).
//! - **Gravity**: an impulse applied to a particle's velocity on each physical move, scaled by its
//!   mass, turning the velocity toward down. New particles start out intending to move down.
//! - **Vacuum** is empty capacity, and most of a ball pit: a particle moves only into vacuum, and
//!   leaves vacuum behind. Evaluate favors heavier particles. Nothing is overwritten, swapped or
//!   displaced. A particle may still offer into a cell holding another (air, water or sand, each
//!   weighted), which evaluate rejects.
//! - **Slate** is static, like bedrock: nothing offers into it.

use crate::{Dir, Material, Velocity};

/// The tunables of the physics distribution. All integers, so the shaders match bit for bit.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Physics {
    /// Gravity's impulse per unit of mass, in velocity units (`Velocity::SCALE` is one cell per move).
    pub gravity: u32,
    /// Each material's mass: air, water, sand. Vacuum has none; slate and bedrock are static.
    pub mass_air: u32,
    pub mass_water: u32,
    pub mass_sand: u32,
    /// The weight multiplier for a neighbor holding air, water, sand or vacuum.
    pub into_air: u32,
    pub into_water: u32,
    pub into_sand: u32,
    pub into_vacuum: u32,
}

impl Default for Physics {
    fn default() -> Self {
        Physics {
            gravity: 64,
            mass_air: 1,
            mass_water: 2,
            mass_sand: 4,
            into_air: 16,
            into_water: 8,
            into_sand: 1,
            into_vacuum: 16,
        }
    }
}

/// The largest a weight multiplier or mass counts for, so six weights sum within a `u32`.
const FACTOR_CAP: u32 = 255;
const MASS_CAP: u32 = 16;
/// The largest `along` counts for: a steady-scale velocity reaches at most 3 * `SCALE`.
const ALONG_CAP: u32 = 1 << 10;

impl Physics {
    pub fn mass(&self, m: Material) -> u32 {
        match m {
            Material::Air => self.mass_air,
            Material::Water => self.mass_water,
            Material::Sand => self.mass_sand,
            Material::Bedrock | Material::Slate | Material::Vacuum => 0,
        }
    }

    /// Whether a particle of `origin` may offer into a cell holding `target` at all: the exclusion
    /// step. Only air, water and sand move; they offer into vacuum, or into another such particle
    /// whose weight multiplier isn't zero; nothing offers into slate or bedrock.
    pub fn compatible(&self, origin: Material, target: Material) -> bool {
        origin.moves() && (target == Material::Vacuum || (target.moves() && self.factor(target) > 0))
    }

    /// Evaluate's conditions: whether a particle of `origin` may move into a cell holding `target`.
    /// Only into vacuum.
    pub fn accepts(&self, origin: Material, target: Material) -> bool {
        origin.moves() && target == Material::Vacuum
    }

    /// The weight of moving a particle of `m` with `velocity` toward `d`, into a cell holding
    /// `target`: zero away from the velocity, growing as the square of how far it points that way.
    pub fn weight(&self, m: Material, velocity: Velocity, d: Dir, target: Material) -> u32 {
        let factor = self.factor(target);
        let along = (velocity.along(d).max(0) as u32).min(ALONG_CAP);
        factor * self.mass(m).min(MASS_CAP) * ((along * along) >> 8)
    }

    /// The weight multiplier for offering into a cell holding `target`.
    pub fn factor(&self, target: Material) -> u32 {
        match target {
            Material::Air => self.into_air,
            Material::Water => self.into_water,
            Material::Sand => self.into_sand,
            Material::Vacuum => self.into_vacuum,
            Material::Bedrock | Material::Slate => 0,
        }
        .min(FACTOR_CAP)
    }

    /// The velocity of a particle of `m` that just moved: gravity's impulse added, then brought back
    /// to the steady scale, keeping its direction.
    pub fn moved(&self, m: Material, v: Velocity) -> Velocity {
        let impulse = (self.gravity.min(1 << 12) * self.mass(m).min(MASS_CAP)) as i32;
        Velocity::steady(v.x as i32, v.y as i32 + impulse)
    }
}

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
