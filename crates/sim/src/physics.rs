//! The physics distribution: the overlay on selection that evaluate and propose apply. It holds
//! only universal truths: constants (the tunables below) and pure functions of a cell's live state
//! and its neighbors' published state. It excludes options and biases the weights of the rest. It
//! changes no state itself: a cell's state changes only through a contract's effects (`contract`),
//! which may draw on the truths here, such as gravity.
//!
//! What it holds so far:
//! - **Capacity** (the hard limit in evaluate): a cell holds one particle at most, so a cell holding
//!   a particle rejects every offer of a particle. A cell holding nothing can take one.
//! - **Movement** (propose): a particle intends to move along its velocity. From the six neighbors,
//!   propose excludes those whose published state is incompatible with the particle and weights the
//!   rest by how well they line up with the velocity, by the particle's mass, and by what the
//!   neighbor holds (vacuum and air most amenable, then water, then sand).
//! - **Gravity**: an impulse applied to a particle's velocity on each physical move, scaled by its
//!   mass, turning it toward down (`moved`).
//!
//! Provisional, marked where they live: the hard 90° cutoff on direction in `weight`, and every
//! material sharing one directional profile.

use crate::{Dir, Material, Velocity};

/// The tunables of the physics distribution: constants, all integers so the shaders match bit for
/// bit. They include the deflection constants that the provisional rejection effect reads
/// (`contract::deflected`).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Physics {
    /// Gravity's impulse per unit of mass, in velocity units (`Velocity::SCALE` is one cell per move).
    pub gravity: u32,
    /// Each material's mass: air (very low), water (low), sand (mid). Vacuum is nothing and has none.
    /// Slate and bedrock are static; their masses (high, very high) aren't read by any rule yet.
    pub mass_air: u32,
    pub mass_water: u32,
    pub mass_sand: u32,
    /// The weight multiplier for proposing into a neighbor holding air, water, sand or vacuum.
    pub into_air: u32,
    pub into_water: u32,
    pub into_sand: u32,
    pub into_vacuum: u32,
    /// Each material's deflection, for the provisional rejection effect: how far a rejection turns
    /// its velocity, in velocity units.
    pub deflect_air: u32,
    pub deflect_water: u32,
    pub deflect_sand: u32,
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
            deflect_air: 64,
            deflect_water: 32,
            deflect_sand: 2,
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

    /// Propose's exclusion: whether a particle of `origin` may offer into a neighbor whose published
    /// state holds `target`. Only particles that move offer. They may offer into a neighbor holding
    /// nothing, or holding another particle that moves (whose weight multiplier isn't zero), since
    /// that neighbor may have changed by the time it evaluates. Nothing offers into a static particle.
    pub fn compatible(&self, origin: Material, target: Material) -> bool {
        origin.moves() && (target == Material::Vacuum || (target.moves() && self.factor(target) > 0))
    }

    /// Evaluate's exclusion, the capacity limit: whether a cell whose live state holds `holding`
    /// entertains an offer of a particle of `offered`. A cell holds one particle at most: holding a
    /// particle, it rejects every offer of one; holding nothing, it can take one.
    pub fn entertains(&self, offered: Material, holding: Material) -> bool {
        offered.moves() && !holding.is_particle()
    }

    /// The weight of moving a particle of `m` with `velocity` toward `d`, into a cell holding
    /// `target`: growing as the square of how far the move points along the velocity, times the
    /// particle's mass, times what the target holds.
    ///
    /// Provisional: anything at or behind 90° from the velocity weighs zero, for every material.
    /// The doc has a backward move only unlikely, and lets air be more permissive than sand.
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

    /// Gravity: the velocity of a particle of `m` that just moved, with gravity's impulse added,
    /// then brought back to the steady scale.
    pub fn moved(&self, m: Material, v: Velocity) -> Velocity {
        let impulse = (self.gravity.min(1 << 12) * self.mass(m).min(MASS_CAP)) as i32;
        Velocity::steady(v.x as i32, v.y as i32 + impulse)
    }
}
