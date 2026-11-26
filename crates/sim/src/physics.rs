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
//!   and a cell holding air, which is insubstantial, a substantial one, unless that air is tied up
//!   in an offer of its own (`entertains`).
//! - **Movement** (propose): a particle intends to move along its velocity. Which neighbors are
//!   incompatible with it? Only the physically impossible: nothing, or a static particle, offers,
//!   and nothing offers into a static particle (`offers_to`). Which does it most prefer? Weights
//!   follow how well a move lines up with the velocity, halving with the turn from it as many times
//!   as the particle's mass and its velocity's magnitude say (a heavy, fast particle all but never
//!   offers backward), times its mass, and how amenable it is to what the neighbor holds (`weight`).
//! - **Velocity** is a vector: a direction and a magnitude. Impulses add to it, and its magnitude
//!   approaches the `ceiling` asymptotically, so a particle intending east hard holds that through
//!   one impulse south and turns south through many (`impelled`).
//! - **Air and vacuum**: when a cell holding air shares a neighborhood with a cell holding vacuum,
//!   the air offers into the vacuum (`air_meets_vacuum`). On acceptance it spawns itself a new air
//!   (`contract`).
//! - **Gravity**: an impulse applied to a particle's velocity on each physical move, scaled by its
//!   mass (`moved`).

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
}

/// The physics distribution's data: gravity and every material's properties.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Physics {
    /// Gravity's impulse per unit of mass, in velocity units (`Velocity::SCALE` is one cell per move).
    pub gravity: u32,
    /// The magnitude a velocity approaches, asymptotically, as impulses add to it.
    pub ceiling: u32,
    /// Each material's properties, indexed by its encoded value (`Material as usize`).
    pub materials: [Properties; 6],
}

impl Default for Physics {
    fn default() -> Self {
        let mut materials = [Properties { mass: 0, into: 0, deflect: 0 }; 6];
        let mut set = |m: Material, mass, into, deflect| materials[m as usize] = Properties { mass, into, deflect };
        // Mass: air very low, water low, sand mid, slate high, bedrock very high, vacuum none.
        set(Material::Vacuum, 0, 16, 0);
        set(Material::Air, 1, 16, 64);
        set(Material::Water, 2, 8, 128);
        set(Material::Sand, 4, 1, 2);
        set(Material::Slate, 8, 0, 0);
        set(Material::Bedrock, 16, 0, 0);
        Physics { gravity: 64, ceiling: 1024, materials }
    }
}

/// The largest an `into` or a mass counts for, so six weights sum within a `u32`.
const INTO_CAP: u32 = 255;
const MASS_CAP: u32 = 16;
/// The heaviest a direction weight can be: a move straight along the velocity.
const WEIGHT_TOP: u32 = 1 << 16;
/// How many halvings of the weight count at most: past it, a move weighs nothing.
const HALVINGS_CAP: u32 = 16;
/// The largest the ceiling counts for.
const CEILING_CAP: u32 = 1 << 13;
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

    pub fn deflection(&self, m: Material) -> u32 {
        self.of(m).deflect.min(DEFLECT_CAP)
    }

    /// Propose's exclusion: which neighbors are incompatible with a particle of `m`? Only the
    /// physically impossible offers: a particle that doesn't move (static, or nothing at all) offers
    /// nothing, and nothing offers into a static particle. A neighbor holding another particle that
    /// moves stays compatible, since it may change its contents by the time it evaluates.
    pub fn offers_to(&self, m: Material, target: Material) -> bool {
        m.moves() && !target.is_static() && self.into(target) > 0
    }

    /// The air and vacuum interaction: whether a cell holding `m` offers only into the vacuum in its
    /// neighborhood. True for air with a vacuum neighbor.
    pub fn air_meets_vacuum(&self, m: Material, seen: &[crate::State; 6]) -> bool {
        m == Material::Air && seen.iter().any(|s| s.material == Material::Vacuum)
    }

    /// Evaluate's exclusion, the capacity limit: whether a cell whose live state holds `holding`
    /// entertains an offer of a particle of `offered`. Holding a substantial particle, it rejects
    /// every offer of one. Holding nothing, it can take any particle. Holding air, which is
    /// insubstantial, it can take a more substantial one, and the air ceases to exist, unless the air
    /// is `committed`: tied up in an offer of its own, its capacity is tied up too.
    pub fn entertains(&self, offered: Material, holding: Material, committed: bool) -> bool {
        let room = !holding.is_particle() || (!holding.is_substantial() && offered.is_substantial() && !committed);
        offered.moves() && room
    }

    /// The weight of moving a particle of `m` with `velocity` toward `d`, into a cell holding
    /// `target`: its preference for the move, times its mass, times how amenable it is to what the
    /// target holds. The preference halves `mass * magnitude / SCALE` times per unit of turn from
    /// the velocity, so straight ahead weighs most, and the heavier and faster the particle the
    /// more firmly it holds to its velocity.
    pub fn weight(&self, m: Material, velocity: Velocity, d: Dir, target: Material) -> u32 {
        self.into(target) * self.mass(m) * self.preference(m, velocity, d)
    }

    /// How much a particle of `m` with `velocity` prefers moving toward `d`: `WEIGHT_TOP` straight
    /// ahead, halving with the turn from the velocity, scaled by mass and magnitude.
    pub fn preference(&self, m: Material, velocity: Velocity, d: Dir) -> u32 {
        let (dx, dy) = d.step();
        let (vx, vy) = (velocity.x as i32, velocity.y as i32);
        let magnitude = isqrt((vx * vx + vy * vy) as u32);
        let norm = isqrt(((vx * vx + vy * vy) * (dx * dx + dy * dy)) as u32) as i32;
        // The turn, 1 - cosine, in 1/1024ths: 0 straight ahead, 2048 straight back.
        let turn = if norm == 0 { 1024 } else { (1024 - velocity.along(d) * 1024 / norm).clamp(0, 2048) as u32 };
        // Halvings, in 1/1024ths of one.
        let halvings = (self.mass(m) * magnitude.min(CEILING_CAP) * turn) >> 8;
        let whole = halvings >> 10;
        if whole >= HALVINGS_CAP {
            return 0;
        }
        let w = WEIGHT_TOP >> whole;
        // Between whole halvings, a straight line from w down to w / 2.
        w - ((w * (halvings & 1023)) >> 11)
    }

    /// Gravity: the velocity of a particle of `m` that just moved, with gravity's impulse added.
    pub fn moved(&self, m: Material, v: Velocity) -> Velocity {
        let impulse = (self.gravity.min(1 << 12) * self.mass(m)) as i32;
        self.impelled(v, 0, impulse)
    }

    /// The velocity `v` with the impulse `(ix, iy)` added. Where the impulse lengthens it, the gain
    /// in magnitude is eased by how far it already is toward the ceiling, so magnitude approaches
    /// the ceiling and never passes it; direction turns in full.
    pub fn impelled(&self, v: Velocity, ix: i32, iy: i32) -> Velocity {
        let c = self.ceiling.clamp(1, CEILING_CAP) as i32;
        let cap = CEILING_CAP as i32;
        let (ux, uy) = (v.x as i32 + ix.clamp(-cap, cap), v.y as i32 + iy.clamp(-cap, cap));
        let before = (isqrt((v.x as i32 * v.x as i32 + v.y as i32 * v.y as i32) as u32) as i32).min(c);
        let after = isqrt((ux * ux + uy * uy) as u32) as i32;
        if after == 0 {
            return Velocity::DOWN;
        }
        let eased = if after > before { before + (after - before) * (c - before) / c } else { after };
        Velocity { x: (ux * eased / after) as i16, y: (uy * eased / after) as i16 }
    }
}

/// The integer square root: the largest `r` with `r * r <= n`.
pub fn isqrt(n: u32) -> u32 {
    let mut r = 0u32;
    let mut bit = 1u32 << 30;
    let mut n = n;
    while bit > n {
        bit >>= 2;
    }
    while bit != 0 {
        if n >= r + bit {
            n -= r + bit;
            r = (r >> 1) + bit;
        } else {
            r >>= 1;
        }
        bit >>= 2;
    }
    r
}
