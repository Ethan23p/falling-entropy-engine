//! The CPU reference simulation. Every rule lives here first, in plain Rust, and is tested
//! natively; the WGSL compute shaders mirror it bit for bit.
//!
//! v01.00 starts from scaffolding: a column-offset hexagonal lattice of cells, each holding air,
//! sand or bedrock, advanced by a synchronizer (a cell updates and advances a phase only once every
//! neighbor has reached its phase), reading only its neighbors' published state and writing only
//! itself. The update has no stages yet: a cell publishes what it held. The model (particles,
//! contracts, the physics distribution) is built on this one piece at a time.

pub mod debug;
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
}

/// What a cell publishes at a phase.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct State {
    pub material: Material,
}

impl State {
    pub const AIR: State = State { material: Material::Air };
    pub const BEDROCK: State = State { material: Material::Bedrock };

    pub fn holding(material: Material) -> State {
        State { material }
    }

    /// The material in bits 0-1. The rest of the state's bits are free for the model.
    pub fn encode(self) -> u32 {
        self.material as u32
    }

    pub fn decode(bits: u32) -> State {
        let material = match bits & 3 {
            0 => Material::Air,
            1 => Material::Sand,
            _ => Material::Bedrock,
        };
        State { material }
    }
}

// The update

/// One cell's update: its next state from its own state, the state it kept back from the phase
/// before, and its neighbors' published state, all at the same phase (outside the lattice reads as
/// bedrock). The cell computes only itself. No stages yet: it publishes what it held.
pub fn next(me: State, _kept: State, _seen: [State; 6], _x: i32, _y: i32, _phase: u32) -> State {
    me
}

// The world

/// A cell keeps two words, one per phase parity: the state it published at a phase goes in the
/// word of that phase's parity, tagged with the phase. A neighbor at phase `p` reads the word of
/// `p`'s parity, which the cell doesn't overwrite until it reaches `p + 2`, so a neighbor one phase
/// ahead still serves it (the kept-back value), and no reader needs another word in step with it.
const STATE_BITS: u32 = 19;
const STATE_MASK: u32 = (1 << STATE_BITS) - 1;
/// The phase is stored in the top 13 bits and wraps; neighbors are never more than one apart.
pub const PHASE_MASK: u32 = (1 << (32 - STATE_BITS)) - 1;

fn word(state: State, phase: u32) -> u32 {
    state.encode() | (phase & PHASE_MASK) << STATE_BITS
}

/// The lattice: two `u32` words per cell (see `STATE_BITS`), cell by cell, row by row.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct World {
    width: i32,
    height: i32,
    cells: Vec<u32>,
}

impl World {
    pub fn new(width: i32, height: i32) -> World {
        // Phase 0 in the even word; the odd word holds phase -1.
        let pair = [word(State::AIR, 0), word(State::AIR, PHASE_MASK)];
        World { width, height, cells: pair.repeat((width * height) as usize) }
    }

    /// Rebuilds a world from raw cell words, e.g. a GPU readback.
    pub fn from_words(width: i32, height: i32, words: Vec<u32>) -> World {
        assert_eq!(words.len(), 2 * (width * height) as usize, "{width}x{height} lattice, two words a cell");
        World { width, height, cells: words }
    }

    pub fn width(&self) -> i32 {
        self.width
    }

    pub fn height(&self) -> i32 {
        self.height
    }

    /// The raw cell words, two per cell, row by row, as the GPU stores them.
    pub fn words(&self) -> &[u32] {
        &self.cells
    }

    pub fn in_bounds(&self, x: i32, y: i32) -> bool {
        (0..self.width).contains(&x) && (0..self.height).contains(&y)
    }

    fn index(&self, x: i32, y: i32) -> usize {
        (y * self.width + x) as usize
    }

    /// Which of the cell's two words is its current one.
    fn current(&self, i: usize) -> usize {
        let (even, odd) = (self.cells[2 * i] >> STATE_BITS, self.cells[2 * i + 1] >> STATE_BITS);
        if even.wrapping_sub(odd) & PHASE_MASK == 1 { 2 * i } else { 2 * i + 1 }
    }

    /// What the cell published at its current phase.
    pub fn cell(&self, x: i32, y: i32) -> State {
        State::decode(self.cells[self.current(self.index(x, y))] & STATE_MASK)
    }

    /// What the cell published the phase before: its kept-back value.
    pub fn kept(&self, x: i32, y: i32) -> State {
        State::decode(self.cells[self.current(self.index(x, y)) ^ 1] & STATE_MASK)
    }

    pub fn material(&self, x: i32, y: i32) -> Material {
        self.cell(x, y).material
    }

    pub fn phase(&self, x: i32, y: i32) -> u32 {
        self.cells[self.current(self.index(x, y))] >> STATE_BITS
    }

    /// How many cells hold `material`.
    pub fn count(&self, material: Material) -> usize {
        (0..self.height).map(|y| (0..self.width).filter(|&x| self.material(x, y) == material).count()).sum()
    }

    /// Whether every cell is at the same phase, as after a whole `step`.
    pub fn uniform(&self) -> bool {
        let p = self.phase(0, 0);
        (0..self.height).all(|y| (0..self.width).all(|x| self.phase(x, y) == p))
    }

    /// The brush, the one source and sink of matter. Matter goes only into a cell holding air;
    /// erasing (painting air) clears anything but bedrock. Call it between phases.
    pub fn paint(&mut self, x: i32, y: i32, material: Material) {
        let here = self.material(x, y);
        let allowed = match material {
            Material::Air => here != Material::Air && here != Material::Bedrock,
            _ => here == Material::Air,
        };
        if allowed {
            self.set(x, y, State::holding(material));
        }
    }

    /// Overwrites the cell's current word, keeping its phase.
    fn set(&mut self, x: i32, y: i32, s: State) {
        let i = self.current(self.index(x, y));
        self.cells[i] = word(s, self.cells[i] >> STATE_BITS);
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
    /// neighbor's word of its own phase's parity, and its own two words. Returns whether it advanced.
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
            let n = self.cells[2 * self.index(nx, ny) + parity];
            if n >> STATE_BITS != phase {
                return false; // behind
            }
            seen[d as usize] = State::decode(n & STATE_MASK);
        }
        let me = State::decode(self.cells[2 * i + parity] & STATE_MASK);
        // The kept-back word, which this update overwrites: no neighbor reads it any more.
        let kept = State::decode(self.cells[2 * i + 1 - parity] & STATE_MASK);
        let ahead = phase.wrapping_add(1) & PHASE_MASK;
        self.cells[2 * i + 1 - parity] = word(next(me, kept, seen, x, y, phase), ahead);
        true
    }

    /// One phase for every cell, row by row. Any order gives the same result. Debug builds check
    /// conservation after every step.
    pub fn step(&mut self) {
        #[cfg(debug_assertions)]
        let before = self.count(Material::Sand);
        for y in 0..self.height {
            for x in 0..self.width {
                self.update(x, y);
            }
        }
        #[cfg(debug_assertions)]
        assert_eq!(before, self.count(Material::Sand), "matter not conserved by a step at phase {}", self.phase(0, 0));
    }
}
