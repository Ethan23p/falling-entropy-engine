//! The CPU reference simulation. Every rule lives here first, in plain Rust,
//! and is tested natively; the WGSL compute shaders mirror it bit for bit.
//!
//! Names follow the Primitives section of the Falling Sand Engine tab:
//! a column-offset hexagonal lattice of cells, each holding sand, slate, bedrock or air,
//! advanced by synchronizer-phasing, moving sand by a directed handshake
//! (offer → accept → transfer) at reach 1 with self-writes only.

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

// Gravity

/// The gravity potential of a cell's centre, in half cells: higher is uphill.
pub fn height(x: i32, y: i32) -> i32 {
    -(2 * y + (x & 1))
}

/// How far a move from `(x, y)` toward `d` drops, in half cells; positive is downhill.
pub fn drop(x: i32, y: i32, d: Dir) -> i32 {
    let (nx, ny) = neighbor(x, y, d);
    height(x, y) - height(nx, ny)
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

const SALT_OFFER: u32 = 1;
const SALT_ACCEPT: u32 = 2;

// Cell state

/// The discriminants are the encoded values, shared with the shaders.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[repr(u8)]
pub enum Material {
    /// Not matter: what a cell holds when it holds no matter.
    Air = 0,
    Sand = 1,
    /// Solid matter that never moves.
    Slate = 2,
    /// Slate that can't be erased. Outside the lattice is bedrock.
    Bedrock = 3,
}

/// What a cell holds this phase.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct State {
    pub material: Material,
    /// The direction this cell is sending its matter to.
    pub offer: Option<Dir>,
    /// The direction this cell is receiving matter from.
    pub accept: Option<Dir>,
}

impl State {
    pub const AIR: State = State { material: Material::Air, offer: None, accept: None };
    pub const SLATE: State = State { material: Material::Slate, offer: None, accept: None };
    pub const BEDROCK: State = State { material: Material::Bedrock, offer: None, accept: None };

    pub fn holding(material: Material) -> State {
        State { material, offer: None, accept: None }
    }

    /// 5 bits: material (bits 0-1: 0 air, 1 sand, 2 slate, 3 bedrock), then one intent (bits 2-4), stored
    /// as 1 + a direction's index. A cell holds at most one intent: sand's offer or air's acceptance.
    pub fn encode(self) -> u32 {
        let dir = |d: Option<Dir>| d.map_or(0, |d| d as u32 + 1);
        self.material as u32 | dir(self.offer.or(self.accept)) << 2
    }

    pub fn decode(bits: u32) -> State {
        let v = bits >> 2 & 7;
        let intent = (v != 0).then(|| Dir::ALL[(v - 1) as usize % 6]);
        match bits & 3 {
            0 => State { material: Material::Air, offer: None, accept: intent },
            1 => State { material: Material::Sand, offer: intent, accept: None },
            2 => State::SLATE,
            _ => State::BEDROCK,
        }
    }
}

// The rule

/// One cell's update: its next state from its own state and what it sees around it, all at
/// the same phase (outside the lattice reads as bedrock). Every cell computes only itself, so
/// the two halves of a transfer agree because they read the same snapshot.
pub fn next(me: State, seen: [State; 6], x: i32, y: i32, phase: u32) -> State {
    let at = |d: Dir| seen[d as usize];

    // Transfer, the giving half: my offer was accepted, so I hold air.
    if let Some(d) = me.offer
        && at(d).accept == Some(d.opposite())
    {
        return State::AIR;
    }

    let mut s = me;
    // Transfer, the receiving half: the offer I accepted still stands, so I take its matter.
    if let Some(d) = me.accept {
        s.accept = None;
        let n = at(d);
        if n.offer == Some(d.opposite()) {
            s.material = n.material;
        }
    }

    match s.material {
        Material::Air => {
            if me.accept.is_none() {
                // Accept: the steepest offer aimed at me wins; ties are hashed.
                let offers = Dir::ALL
                    .into_iter()
                    .filter(|&d| at(d).material == Material::Sand && at(d).offer == Some(d.opposite()));
                s.accept = pick(offers, |d| -drop(x, y, d), hash(x, y, phase, SALT_ACCEPT));
            }
        }
        Material::Sand => {
            // An offer stands while its target holds air and hasn't accepted someone else.
            if let Some(d) = s.offer {
                let n = at(d);
                let open = n.material == Material::Air && (n.accept.is_none() || n.accept == Some(d.opposite()));
                if !open {
                    s.offer = None;
                }
            }
            if s.offer.is_none() {
                // Offer: the steepest downhill neighbor holding air and free to accept; ties are hashed.
                let open = Dir::ALL
                    .into_iter()
                    .filter(|&d| drop(x, y, d) > 0 && at(d).material == Material::Air && at(d).accept.is_none());
                s.offer = pick(open, |d| drop(x, y, d), hash(x, y, phase, SALT_OFFER));
            }
        }
        Material::Slate | Material::Bedrock => {}
    }
    s
}

/// The candidate with the highest `key`, choosing among equals by `bits`.
fn pick(candidates: impl Iterator<Item = Dir>, key: impl Fn(Dir) -> i32, bits: u32) -> Option<Dir> {
    let mut best = [Dir::Up; 6];
    let mut n = 0;
    let mut best_key = i32::MIN;
    for d in candidates {
        let k = key(d);
        if k > best_key {
            best_key = k;
            n = 0;
        }
        if k == best_key {
            best[n] = d;
            n += 1;
        }
    }
    (n > 0).then(|| best[bits as usize % n])
}

// The world

/// Bits of a cell's word: its state, its kept-back value and its phase.
const STATE_BITS: u32 = 5;
const STATE_MASK: u32 = (1 << STATE_BITS) - 1;
const PHASE_SHIFT: u32 = 2 * STATE_BITS;
/// The phase is stored in the remaining 22 bits and wraps; neighbors are never more than one apart.
pub const PHASE_MASK: u32 = (1 << (32 - PHASE_SHIFT)) - 1;

fn pack(state: State, kept_back: State, phase: u32) -> u32 {
    state.encode() | kept_back.encode() << STATE_BITS | (phase & PHASE_MASK) << PHASE_SHIFT
}

/// The lattice: one `u32` word per cell, row by row.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct World {
    width: i32,
    height: i32,
    cells: Vec<u32>,
}

impl World {
    pub fn new(width: i32, height: i32) -> World {
        let air = pack(State::AIR, State::AIR, 0);
        World { width, height, cells: vec![air; (width * height) as usize] }
    }

    pub fn width(&self) -> i32 {
        self.width
    }

    pub fn height(&self) -> i32 {
        self.height
    }

    /// The raw cell words, row by row, as the GPU stores them.
    pub fn words(&self) -> &[u32] {
        &self.cells
    }

    pub fn in_bounds(&self, x: i32, y: i32) -> bool {
        (0..self.width).contains(&x) && (0..self.height).contains(&y)
    }

    fn index(&self, x: i32, y: i32) -> usize {
        (y * self.width + x) as usize
    }

    pub fn cell(&self, x: i32, y: i32) -> State {
        State::decode(self.cells[self.index(x, y)] & STATE_MASK)
    }

    pub fn material(&self, x: i32, y: i32) -> Material {
        self.cell(x, y).material
    }

    pub fn phase(&self, x: i32, y: i32) -> u32 {
        self.cells[self.index(x, y)] >> PHASE_SHIFT
    }

    pub fn count(&self, material: Material) -> usize {
        self.cells.iter().filter(|&&w| State::decode(w & STATE_MASK).material == material).count()
    }

    /// The brush, the one source and sink of matter: the cell now holds `material` and nothing
    /// in flight. Matter goes only into a cell holding air; erasing (painting air) clears anything
    /// but bedrock. Call it between phases.
    pub fn paint(&mut self, x: i32, y: i32, material: Material) {
        let here = self.material(x, y);
        let allowed = if material == Material::Air { here != Material::Bedrock } else { here == Material::Air };
        if !allowed {
            return;
        }
        let i = self.index(x, y);
        let s = State::holding(material);
        self.cells[i] = pack(s, s, self.cells[i] >> PHASE_SHIFT);
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

    /// Evaluate one cell. It advances one phase unless a neighbor is still behind it, reading
    /// neighbors a phase ahead from their kept-back values. Returns whether it advanced.
    pub fn update(&mut self, x: i32, y: i32) -> bool {
        let word = self.cells[self.index(x, y)];
        let phase = word >> PHASE_SHIFT;
        let mut seen = [State::BEDROCK; 6];
        for d in Dir::ALL {
            let (nx, ny) = neighbor(x, y, d);
            if !self.in_bounds(nx, ny) {
                continue;
            }
            let n = self.cells[self.index(nx, ny)];
            let ahead = (n >> PHASE_SHIFT).wrapping_sub(phase) & PHASE_MASK;
            let bits = match ahead {
                0 => n,
                1 => n >> STATE_BITS,
                _ => return false, // behind
            };
            seen[d as usize] = State::decode(bits & STATE_MASK);
        }
        let me = State::decode(word & STATE_MASK);
        let i = self.index(x, y);
        self.cells[i] = pack(next(me, seen, x, y, phase), me, phase + 1);
        true
    }

    /// One phase for every cell, row by row. Any order gives the same result.
    pub fn step(&mut self) {
        for y in 0..self.height {
            for x in 0..self.width {
                self.update(x, y);
            }
        }
    }
}
