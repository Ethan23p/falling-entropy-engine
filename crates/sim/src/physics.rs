//! The physics distribution: the overlay on selection that evaluate and propose apply. It
//! excludes the options whose conditions the particles' materials can't meet, and biases the
//! weights of the rest from the hosted particle's velocity and what the other cell holds.
//!
//! v01.00's policies:
//! - **Movement**: a cell hosting a particle proposes to move it toward a neighbor, weighted by
//!   how far the particle's velocity points that way. A particle at rest has no lean.
//! - **Gravity**: an impulse down added to a particle's velocity on each physical move, scaled by
//!   its mass. It reaches the distribution through velocity.
//! - **Overwriting**: sand may offer into a cell holding air (a toggle); the air is overwritten,
//!   and the origin is granted an air. Sand may also offer into sand, which evaluate rejects.

use crate::{Dir, Material, Velocity};

/// The tunables of the physics distribution. All integers, so the shaders match bit for bit.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Physics {
    /// Whether sand may offer into a cell holding air.
    pub sand_into_air: bool,
    /// Gravity's impulse per unit of mass, in velocity units (256 is one cell per move).
    pub gravity: u32,
    /// Each material's mass: air, sand. Bedrock is static and never moves.
    pub mass_air: u32,
    pub mass_sand: u32,
    /// The weight every compatible neighbor gets, before velocity's bias.
    pub restless: u32,
    /// The weight added per cell per move of velocity along a direction.
    pub align: u32,
    /// The weight multiplier for a neighbor holding air, and one holding sand.
    pub into_air: u32,
    pub into_sand: u32,
}

impl Default for Physics {
    fn default() -> Self {
        Physics {
            sand_into_air: true,
            gravity: 64,
            mass_air: 1,
            mass_sand: 4,
            restless: 4,
            align: 16,
            into_air: 16,
            into_sand: 1,
        }
    }
}

/// The largest a single weight's velocity term gets, so six weights sum within a `u32`.
const ALIGN_CAP: u32 = 1 << 20;
/// The largest a weight multiplier gets.
const FACTOR_CAP: u32 = 255;

impl Physics {
    pub fn mass(&self, m: Material) -> u32 {
        match m {
            Material::Air => self.mass_air,
            Material::Sand => self.mass_sand,
            Material::Bedrock => 0,
        }
    }

    /// Whether a particle of `origin` may offer into a cell holding `target` at all: the exclusion
    /// step. Air and bedrock offer nowhere; nothing offers into bedrock.
    pub fn compatible(&self, origin: Material, target: Material) -> bool {
        match (origin, target) {
            (Material::Sand, Material::Air) => self.sand_into_air,
            (Material::Sand, Material::Sand) => self.into_sand > 0,
            _ => false,
        }
    }

    /// Whether evaluate's conditions let `origin` overwrite what the target holds: sand overwrites
    /// air. Anything else is rejected.
    pub fn overwrites(&self, origin: Material, target: Material) -> bool {
        origin == Material::Sand && target == Material::Air && self.sand_into_air
    }

    /// The weight of moving a particle with `velocity` toward `d`, into a cell holding `target`.
    pub fn weight(&self, velocity: Velocity, d: Dir, target: Material) -> u32 {
        let factor = match target {
            Material::Air => self.into_air,
            Material::Sand => self.into_sand,
            Material::Bedrock => 0,
        }
        .min(FACTOR_CAP);
        let along = velocity.along(d).max(0) as u32;
        let lean = ((along.min(ALIGN_CAP) * self.align.min(255)) >> 8).min(ALIGN_CAP);
        factor * (self.restless.min(ALIGN_CAP) + lean)
    }

    /// The velocity of a particle of `m` that just moved: gravity's impulse added, saturating.
    pub fn moved(&self, m: Material, v: Velocity) -> Velocity {
        let impulse = (self.gravity.min(1 << 12) * self.mass(m).min(1 << 4)) as i32;
        Velocity { x: v.x, y: (v.y as i32 + impulse).clamp(i16::MIN as i32, i16::MAX as i32) as i16 }
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
