//! The physics distribution: the overlay on selection that evaluate and propose apply. It holds
//! only universal truths: data (each material's properties, and gravity) and pure functions that
//! compare those values over a cell's live state and its neighbors' published state. It excludes
//! options and biases the weights of the rest, so every probability is a distribution conditioned
//! on that data. It changes no state itself: a cell's state changes only through a contract's
//! effects (`contract`), which may draw on the truths here.
//!
//! What it holds so far:
//! - **Capacity** (evaluate's exclusion): a cell holds one particle at most, so a cell holding a
//!   substantial particle rejects every offer of one. A cell holding nothing can take any particle,
//!   and a cell holding air, which is insubstantial, a substantial one (`entertains`).
//! - **Movement** (propose): a particle intends to move along its velocity. What might a cell holding
//!   it offer to? Propose excludes the neighbors it wouldn't (`offers_to`) and weights the rest
//!   by how well they line up with the velocity, softened by the material's directional tolerance
//!   (a move against the velocity is unlikely, never impossible), by the particle's mass, and by
//!   how amenable it is to what the neighbor holds (`weight`).
//! - **Gravity**: an impulse applied to a particle's velocity on each physical move, scaled by its
//!   mass, turning it toward down (`moved`).

use crate::{Dir, Material, Velocity};

/// A material's tunable properties: intrinsic constants, all integers so the shaders match bit for
/// bit. (`static` is the other material property, and isn't tunable: `Material::is_static`.)
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Properties {
    /// How much force it accumulates and how it settles relative to other particles. Vacuum, being
    /// nothing, has none.
    pub mass: u32,
    /// How amenable a particle is to offering into a cell holding this: the weight multiplier for
    /// that offer, the same for every offering particle so far. Zero excludes it.
    pub into: u32,
    /// How far a rejection turns this particle's intent, in velocity units: the deflection effect
    /// (`contract::Effect::Deflect`).
    pub deflect: u32,
    /// How allowing this particle is with its directionality: the weight a move gets on top of how
    /// well it lines up with the velocity, so even a move straight against it stays possible. Sand
    /// keeps close to its intent; air goes almost any which way.
    pub tolerance: u32,
}

/// The physics distribution's data: gravity and every material's properties.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Physics {
    /// Gravity's impulse per unit of mass, in velocity units (`Velocity::SCALE` is one cell per move).
    pub gravity: u32,
    /// Each material's properties, indexed by its encoded value (`Material as usize`).
    pub materials: [Properties; 6],
}

impl Default for Physics {
    fn default() -> Self {
        let mut materials = [Properties { mass: 0, into: 0, deflect: 0, tolerance: 0 }; 6];
        let mut set = |m: Material, mass, into, deflect, tolerance| {
            materials[m as usize] = Properties { mass, into, deflect, tolerance }
        };
        // Mass: air very low, water low, sand mid, slate high, bedrock very high, vacuum none.
        set(Material::Vacuum, 0, 16, 0, 0);
        set(Material::Air, 1, 16, 64, 64);
        set(Material::Water, 2, 8, 32, 4);
        set(Material::Sand, 4, 1, 2, 0);
        set(Material::Slate, 8, 0, 0, 0);
        set(Material::Bedrock, 16, 0, 0, 0);
        Physics { gravity: 64, materials }
    }
}

/// The largest an `into` or a mass counts for, so six weights sum within a `u32`.
const INTO_CAP: u32 = 255;
const MASS_CAP: u32 = 16;
/// The largest `along` counts for: a steady-scale velocity reaches at most 3 * `SCALE`.
const ALONG_CAP: u32 = 1 << 10;
/// The largest a tolerance counts for.
const TOLERANCE_CAP: u32 = 1 << 10;
/// The largest a deflection counts for.
const DEFLECT_CAP: u32 = 1 << 10;

impl Physics {
    /// A material's properties.
    pub fn of(&self, m: Material) -> Properties {
        self.materials[m as usize]
    }

    /// A material's properties, for tuning.
    pub fn of_mut(&mut self, m: Material) -> &mut Properties {
        &mut self.materials[m as usize]
    }

    pub fn mass(&self, m: Material) -> u32 {
        self.of(m).mass.min(MASS_CAP)
    }

    /// How amenable a particle is to offering into a cell holding `target`.
    pub fn into(&self, target: Material) -> u32 {
        self.of(target).into.min(INTO_CAP)
    }

    pub fn tolerance(&self, m: Material) -> u32 {
        self.of(m).tolerance.min(TOLERANCE_CAP)
    }

    pub fn deflection(&self, m: Material) -> u32 {
        self.of(m).deflect.min(DEFLECT_CAP)
    }

    /// Propose's exclusion: what might a cell holding a particle of `m` offer to? The neighbor whose
    /// published state holds `target`, when the particle moves (nothing, or a static particle,
    /// offers nothing) and the neighbor holds nothing, or a particle that moves and that this one is
    /// amenable to (`into` above zero), since that neighbor may have changed by the time it
    /// evaluates. Direction excludes nothing: it only weighs (`weight`).
    pub fn offers_to(&self, m: Material, target: Material) -> bool {
        let amenable = !target.is_particle() || (target.moves() && self.into(target) > 0);
        m.moves() && amenable
    }

    /// Evaluate's exclusion, the capacity limit: whether a cell whose live state holds `holding`
    /// entertains an offer of a particle of `offered`. Holding a substantial particle, it rejects
    /// every offer of one. Holding nothing, it can take any particle. Holding air, which is
    /// insubstantial, it can take a more substantial one, and the air ceases to exist.
    pub fn entertains(&self, offered: Material, holding: Material) -> bool {
        let room = !holding.is_particle() || (!holding.is_substantial() && offered.is_substantial());
        offered.moves() && room
    }

    /// The weight of moving a particle of `m` with `velocity` toward `d`, into a cell holding
    /// `target`: how far the move points along the velocity (growing as its square, zero at 90° and
    /// beyond) plus the material's directional tolerance, times the particle's mass, times how
    /// amenable it is to what the target holds.
    pub fn weight(&self, m: Material, velocity: Velocity, d: Dir, target: Material) -> u32 {
        let along = (velocity.along(d).max(0) as u32).min(ALONG_CAP);
        let direction = ((along * along) >> 8) + self.tolerance(m);
        self.into(target) * self.mass(m) * direction
    }

    /// Gravity: the velocity of a particle of `m` that just moved, with gravity's impulse added,
    /// then brought back to the steady scale.
    pub fn moved(&self, m: Material, v: Velocity) -> Velocity {
        let impulse = (self.gravity.min(1 << 12) * self.mass(m)) as i32;
        Velocity::steady(v.x as i32, v.y as i32 + impulse)
    }
}
