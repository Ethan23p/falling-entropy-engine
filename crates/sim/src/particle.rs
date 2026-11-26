//! Particles and their material properties.
//!
//! A particle is an individual physics body: a material and a velocity. It is located relative to
//! the cell that holds it (one particle per cell, at most, for now), so it has no position of its
//! own. A material's properties are intrinsic constants: its mass and the like (tunables of the
//! physics distribution, `physics::Properties`), whether it is static, and whether it is
//! substantial.

/// What a cell holds. The discriminants are the encoded values, shared with the shaders.
///
/// Vacuum is listed with the materials because a cell has to say what it holds even when that is
/// nothing: vacuum is nothing, no particle and no mass. Air is a particle, of very low mass, and
/// insubstantial.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[repr(u8)]
pub enum Material {
    /// A particle of very low mass, and insubstantial: a more substantial particle may transfer into
    /// a cell holding air, and the air ceases to exist.
    Air = 0,
    /// A particle of mid mass.
    Sand = 1,
    /// Static. The boundary: it can't be erased. Outside the lattice reads as bedrock.
    Bedrock = 2,
    /// Nothing: no particle, no mass.
    Vacuum = 3,
    /// Static. Unlike bedrock, the brush can erase it.
    Slate = 4,
    /// A particle of low mass.
    Water = 5,
}

impl Material {
    /// Every material, in encoded order: the order of a census.
    pub const ALL: [Material; 6] =
        [Material::Air, Material::Sand, Material::Bedrock, Material::Vacuum, Material::Slate, Material::Water];

    /// Whether this is a particle at all: anything but vacuum.
    pub const fn is_particle(self) -> bool {
        !matches!(self, Material::Vacuum)
    }

    /// Whether this is a substantial particle: anything but vacuum and air. Air is insubstantial, so
    /// it isn't conserved: it can cease to exist, and spawn anew (see `contract`).
    pub const fn is_substantial(self) -> bool {
        self.is_particle() && !matches!(self, Material::Air)
    }

    /// The material property `static`: slate and bedrock never move.
    pub const fn is_static(self) -> bool {
        matches!(self, Material::Slate | Material::Bedrock)
    }

    /// Whether this is a particle that moves: one that isn't static (air, water, sand).
    pub const fn moves(self) -> bool {
        self.is_particle() && !self.is_static()
    }

    pub fn decode(bits: u32) -> Material {
        Material::ALL.get(bits as usize).copied().unwrap_or(Material::Vacuum)
    }
}

/// A particle's velocity: its intent to move, a direction finer than the six sides of a hexagon,
/// `y` growing downward.
///
/// For now velocity is basically a direction: it keeps a steady scale (`SCALE`, one cell per move,
/// measured as the larger of `|x|` and `|y|`) rather than a magnitude of its own.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct Velocity {
    pub x: i16,
    pub y: i16,
}

impl Velocity {
    pub const SCALE: i32 = 256;
    /// What vacuum and static particles carry.
    pub const ZERO: Velocity = Velocity { x: 0, y: 0 };
    /// What a new particle intends: to move down. Everything always intends to move.
    pub const DOWN: Velocity = Velocity { x: 0, y: Velocity::SCALE as i16 };

    /// The direction of `(x, y)` at the steady scale; down when it has none.
    pub fn steady(x: i32, y: i32) -> Velocity {
        let m = x.abs().max(y.abs());
        if m == 0 {
            return Velocity::DOWN;
        }
        Velocity { x: (x * Velocity::SCALE / m) as i16, y: (y * Velocity::SCALE / m) as i16 }
    }

    /// How far this points toward `d`: the dot product with `d`'s step (see `Dir::step`).
    pub fn along(self, d: crate::Dir) -> i32 {
        let (dx, dy) = d.step();
        self.x as i32 * dx + self.y as i32 * dy
    }

    pub fn encode(self) -> u32 {
        self.x as u16 as u32 | (self.y as u16 as u32) << 16
    }

    pub fn decode(bits: u32) -> Velocity {
        Velocity { x: bits as u16 as i16, y: (bits >> 16) as u16 as i16 }
    }
}
