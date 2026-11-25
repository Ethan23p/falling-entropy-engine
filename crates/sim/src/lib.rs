//! The CPU reference simulation. Every rule lives here first, in plain Rust, and is tested
//! natively; the WGSL compute shaders mirror it bit for bit.
//!
//! A column-offset hexagonal lattice of cells, advanced by a synchronizer (a cell updates and
//! advances a phase only once every neighbor has reached its phase). Each cell holds one particle
//! (air, water, sand, slate or bedrock with a velocity, or vacuum) and its contracts, reads only its neighbors' published
//! state and writes only itself. The update's stages are in `update`; the physics distribution
//! that biases selection is in `physics`.

pub mod debug;
pub mod physics;
mod update;

pub use physics::Physics;
#[cfg(test)]
mod tests;

// Geometry

/// The six neighbor directions of a column-offset hexagonal lattice, clockwise from up.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[repr(u8)]
pub enum Dir {
    Up = 0,
    UpRight = 1,
    DownRight = 2,
    Down = 3,
    DownLeft = 4,
    UpLeft = 5,
}

impl Dir {
    pub const ALL: [Dir; 6] = [Dir::Up, Dir::UpRight, Dir::DownRight, Dir::Down, Dir::DownLeft, Dir::UpLeft];

    pub fn opposite(self) -> Dir {
        Dir::ALL[(self as usize + 3) % 6]
    }

    /// The step toward this neighbor, in half cells, `y` growing downward: what velocity is
    /// measured against. Up and down are 2 long; the diagonals 2 across and 1 up or down.
    pub fn step(self) -> (i32, i32) {
        match self {
            Dir::Up => (0, -2),
            Dir::UpRight => (2, -1),
            Dir::DownRight => (2, 1),
            Dir::Down => (0, 2),
            Dir::DownLeft => (-2, 1),
            Dir::UpLeft => (-2, -1),
        }
    }
}

/// The neighbor of `(x, y)` toward `d`. Odd columns sit half a cell lower than even ones,
/// and `y` grows downward.
pub fn neighbor(x: i32, y: i32, d: Dir) -> (i32, i32) {
    let odd = x & 1;
    match d {
        Dir::Up => (x, y - 1),
        Dir::Down => (x, y + 1),
        Dir::UpRight => (x + 1, y - 1 + odd),
        Dir::DownRight => (x + 1, y + odd),
        Dir::DownLeft => (x - 1, y + odd),
        Dir::UpLeft => (x - 1, y - 1 + odd),
    }
}

// Randomness

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

// Cell state

/// The discriminants are the encoded values, shared with the shaders.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[repr(u8)]
pub enum Material {
    Air = 0,
    Sand = 1,
    /// The boundary: it can't be erased. Outside the lattice is bedrock.
    Bedrock = 2,
    /// Empty capacity: no particle, no mass.
    Vacuum = 3,
    /// Static: it never moves, and nothing moves into it. Unlike bedrock, the brush can erase it.
    Slate = 4,
    /// A liquid: lighter than sand, heavier than air.
    Water = 5,
}

impl Material {
    /// Every material, in encoded order: the order of a census.
    pub const ALL: [Material; 6] =
        [Material::Air, Material::Sand, Material::Bedrock, Material::Vacuum, Material::Slate, Material::Water];

    /// Whether this is a particle that moves: air, sand or water.
    pub fn moves(self) -> bool {
        matches!(self, Material::Air | Material::Sand | Material::Water)
    }

    pub fn decode(bits: u32) -> Material {
        Material::ALL.get(bits as usize).copied().unwrap_or(Material::Vacuum)
    }
}

/// A particle's velocity: its intent to move, a direction finer than the six sides of a hexagon,
/// at a steady scale for now (`SCALE`, one cell per move, measured as the larger of `|x|` and
/// `|y|`), `y` growing downward.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct Velocity {
    pub x: i16,
    pub y: i16,
}

impl Velocity {
    pub const SCALE: i32 = 256;
    pub const ZERO: Velocity = Velocity { x: 0, y: 0 };
    /// What a new particle intends: to move down.
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
    pub fn along(self, d: Dir) -> i32 {
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

/// What a cell publishes at a phase: the particle it holds (or vacuum), and its contracts.
///
/// - `offer`: an offer this cell proposed in its last update, to move its particle to the
///   neighbor toward that direction. The neighbor evaluates it in its next update, and this cell
///   resolves it in the update after, reading the verdict. Meanwhile the particle is committed.
/// - `accept`: the verdict on an offer from the neighbor toward that direction, accepted in this
///   cell's last update; every other offer that targeted this cell then was rejected. The particle
///   held here is in transport until that neighbor resolves, which leaves it vacuum.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct State {
    pub material: Material,
    pub velocity: Velocity,
    pub offer: Option<Dir>,
    pub accept: Option<Dir>,
}

impl State {
    pub const AIR: State = State::holding(Material::Air);
    pub const BEDROCK: State = State::holding(Material::Bedrock);
    pub const VACUUM: State = State::holding(Material::Vacuum);

    /// A fresh cell with no contracts: a particle that moves intends to move down.
    pub const fn holding(material: Material) -> State {
        let velocity = match material {
            Material::Air | Material::Sand | Material::Water => Velocity::DOWN,
            _ => Velocity::ZERO,
        };
        State { material, velocity, offer: None, accept: None }
    }

    /// Bits 0-2 the material, 3-5 the offer and 6-8 the acceptance (1 + a direction, or 0 for
    /// none). The velocity has a word of its own (`Velocity::encode`).
    pub fn encode(self) -> u32 {
        let dir = |d: Option<Dir>| d.map_or(0, |d| d as u32 + 1);
        self.material as u32 | dir(self.offer) << 3 | dir(self.accept) << 6
    }

    pub fn decode(bits: u32, velocity: u32) -> State {
        let dir = |b: u32| match b & 7 {
            0 | 7 => None,
            n => Some(Dir::ALL[n as usize - 1]),
        };
        State {
            material: Material::decode(bits & 7),
            velocity: Velocity::decode(velocity),
            offer: dir(bits >> 3),
            accept: dir(bits >> 6),
        }
    }

    /// What the cell counts as holding in a census of published state: a particle accepted this
    /// phase is still in transport (its origin counts it until it resolves), so the cell counts
    /// as what it held before: vacuum.
    pub fn counts_as(self) -> Material {
        if self.accept.is_some() { Material::Vacuum } else { self.material }
    }
}

// The update

pub use update::next;

// The world

/// A cell keeps four words: per phase parity, a state word and a velocity word. The state it
/// published at a phase goes in that parity's state word, tagged with the phase, and its velocity in
/// that parity's velocity word. A neighbor at phase `p` reads the words of `p`'s parity, which the
/// cell doesn't overwrite until it reaches `p + 2`, so a neighbor one phase ahead still serves it
/// (the kept-back value), and no reader needs another word in step with it.
const STATE_BITS: u32 = 19;
const STATE_MASK: u32 = (1 << STATE_BITS) - 1;
/// The phase is stored in the top 13 bits and wraps; neighbors are never more than one apart.
pub const PHASE_MASK: u32 = (1 << (32 - STATE_BITS)) - 1;
/// Words per cell: state even, state odd, velocity even, velocity odd.
pub const WORDS: usize = 4;

fn word(state: State, phase: u32) -> u32 {
    state.encode() | (phase & PHASE_MASK) << STATE_BITS
}

/// The lattice: four `u32` words per cell (see `WORDS`), cell by cell, row by row, and the
/// physics distribution's tunables.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct World {
    width: i32,
    height: i32,
    cells: Vec<u32>,
    pub physics: Physics,
}

impl World {
    pub fn new(width: i32, height: i32) -> World {
        // Phase 0 in the even word; the odd word holds phase -1.
        // A fresh lattice is empty space: vacuum.
        let v = State::VACUUM.velocity.encode();
        let cell = [word(State::VACUUM, 0), word(State::VACUUM, PHASE_MASK), v, v];
        World { width, height, cells: cell.repeat((width * height) as usize), physics: Physics::default() }
    }

    /// Rebuilds a world from raw cell words, e.g. a GPU readback.
    pub fn from_words(width: i32, height: i32, words: Vec<u32>) -> World {
        assert_eq!(words.len(), WORDS * (width * height) as usize, "{width}x{height} lattice, {WORDS} words a cell");
        World { width, height, cells: words, physics: Physics::default() }
    }

    pub fn width(&self) -> i32 {
        self.width
    }

    pub fn height(&self) -> i32 {
        self.height
    }

    /// The raw cell words, `WORDS` per cell, row by row, as the GPU stores them.
    pub fn words(&self) -> &[u32] {
        &self.cells
    }

    pub fn in_bounds(&self, x: i32, y: i32) -> bool {
        (0..self.width).contains(&x) && (0..self.height).contains(&y)
    }

    fn index(&self, x: i32, y: i32) -> usize {
        (y * self.width + x) as usize
    }

    /// The parity of the cell's current words.
    fn current(&self, i: usize) -> usize {
        let (even, odd) = (self.cells[WORDS * i] >> STATE_BITS, self.cells[WORDS * i + 1] >> STATE_BITS);
        if even.wrapping_sub(odd) & PHASE_MASK == 1 { 0 } else { 1 }
    }

    /// The cell's state as published at the phase of `parity`.
    fn read(&self, i: usize, parity: usize) -> State {
        State::decode(self.cells[WORDS * i + parity] & STATE_MASK, self.cells[WORDS * i + 2 + parity])
    }

    /// What the cell published at its current phase.
    pub fn cell(&self, x: i32, y: i32) -> State {
        let i = self.index(x, y);
        self.read(i, self.current(i))
    }

    /// What the cell published the phase before: its kept-back value.
    pub fn kept(&self, x: i32, y: i32) -> State {
        let i = self.index(x, y);
        self.read(i, self.current(i) ^ 1)
    }

    /// What the cell shows: the material of the particle it holds.
    pub fn material(&self, x: i32, y: i32) -> Material {
        self.cell(x, y).material
    }

    pub fn phase(&self, x: i32, y: i32) -> u32 {
        let i = self.index(x, y);
        self.cells[WORDS * i + self.current(i)] >> STATE_BITS
    }

    /// A census of published state: how many particles of `material` there are, counting a
    /// particle in transport once, at its origin (see `State::counts_as`).
    pub fn count(&self, material: Material) -> usize {
        (0..self.height).map(|y| (0..self.width).filter(|&x| self.cell(x, y).counts_as() == material).count()).sum()
    }

    /// The census of every material, in `Material::ALL` order.
    pub fn census(&self) -> [usize; 6] {
        Material::ALL.map(|m| self.count(m))
    }

    /// Whether every cell is at the same phase, as after a whole `step`.
    pub fn uniform(&self) -> bool {
        let p = self.phase(0, 0);
        (0..self.height).all(|y| (0..self.width).all(|x| self.phase(x, y) == p))
    }

    /// Whether the cell is part of a live contract: it has an offer out, one awaiting its verdict,
    /// or a particle in transport. The brush leaves such cells alone.
    pub fn engaged(&self, x: i32, y: i32) -> bool {
        let (now, kept) = (self.cell(x, y), self.kept(x, y));
        now.offer.is_some() || now.accept.is_some() || kept.offer.is_some()
    }

    /// The brush, the one source and sink of matter. Matter (air, water, sand, slate, bedrock) goes
    /// only into a cell holding vacuum; erasing (painting vacuum) empties anything but bedrock. A
    /// painted particle intends to move down (`State::holding`). Cells engaged in a contract are
    /// left alone. Call it between phases.
    pub fn paint(&mut self, x: i32, y: i32, material: Material) {
        let here = self.material(x, y);
        let allowed =
            here != material && here != Material::Bedrock && (material == Material::Vacuum || here == Material::Vacuum);
        if allowed && !self.engaged(x, y) {
            self.set(x, y, State::holding(material));
        }
    }

    /// Overwrites the cell's current words, keeping its phase.
    fn set(&mut self, x: i32, y: i32, s: State) {
        let i = self.index(x, y);
        let parity = self.current(i);
        let slot = WORDS * i + parity;
        self.cells[slot] = word(s, self.cells[slot] >> STATE_BITS);
        self.cells[WORDS * i + 2 + parity] = s.velocity.encode();
    }

    /// One brush stamp: `paint` every cell whose centre lies within `radius` of `(cx, cy)`, in
    /// cell units, where a cell's centre is `(x, y + 0.5)` in odd columns and `(x, y)` in even ones.
    pub fn stamp(&mut self, cx: f32, cy: f32, radius: f32, material: Material) {
        for y in 0..self.height {
            for x in 0..self.width {
                let (dx, dy) = (x as f32 - cx, y as f32 + 0.5 * (x & 1) as f32 - cy);
                if dx * dx + dy * dy <= radius * radius {
                    self.paint(x, y, material);
                }
            }
        }
    }

    /// Evaluate one cell. It advances one phase unless a neighbor is still behind it, reading each
    /// neighbor's words of its own phase's parity, and its own. Returns whether it advanced.
    pub fn update(&mut self, x: i32, y: i32) -> bool {
        let i = self.index(x, y);
        let phase = self.phase(x, y);
        let parity = (phase & 1) as usize;
        let mut seen = [State::BEDROCK; 6];
        for d in Dir::ALL {
            let (nx, ny) = neighbor(x, y, d);
            if !self.in_bounds(nx, ny) {
                continue;
            }
            let n = self.index(nx, ny);
            if self.cells[WORDS * n + parity] >> STATE_BITS != phase {
                return false; // behind
            }
            seen[d as usize] = self.read(n, parity);
        }
        let me = self.read(i, parity);
        // The kept-back words, which this update overwrites: no neighbor reads them any more.
        let kept = self.read(i, 1 - parity);
        let ahead = phase.wrapping_add(1) & PHASE_MASK;
        let s = next(me, kept, seen, x, y, phase, &self.physics);
        self.cells[WORDS * i + 1 - parity] = word(s, ahead);
        self.cells[WORDS * i + 3 - parity] = s.velocity.encode();
        true
    }

    /// One phase for every cell, row by row. Any order gives the same result. Debug builds check
    /// the census after every step.
    pub fn step(&mut self) {
        #[cfg(debug_assertions)]
        let before = self.census();
        for y in 0..self.height {
            for x in 0..self.width {
                self.update(x, y);
            }
        }
        #[cfg(debug_assertions)]
        assert_eq!(before, self.census(), "particles not conserved by a step at phase {}", self.phase(0, 0));
    }
}
