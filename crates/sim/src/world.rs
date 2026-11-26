//! The world: the lattice of cells as stored, and the synchronizer that advances them.
//!
//! The synchronizer: a cell is static until each of its neighbors has reached its phase; then it
//! updates (`update::next`) and advances a phase. Each cell reads only its neighbors' published
//! state and writes only itself, so cells may update in any order, in place.

use crate::update::next;
use crate::{Dir, Material, Physics, State, neighbor};

/// A cell keeps four words: per phase parity, a state word and a velocity word. The state it
/// published at a phase goes in that parity's state word, tagged with the phase, and its velocity in
/// that parity's velocity word. A neighbor at phase `p` reads the words of `p`'s parity, which the
/// cell doesn't overwrite until it reaches `p + 2`, so a neighbor one phase ahead still serves it
/// (the kept-back value), and no reader needs another word in step with it.
const STATE_BITS: u32 = 19;
pub(crate) const STATE_MASK: u32 = (1 << STATE_BITS) - 1;
/// The phase is stored in the top 13 bits and wraps; neighbors are never more than one apart.
pub const PHASE_MASK: u32 = (1 << (32 - STATE_BITS)) - 1;
/// Words per cell: state even, state odd, velocity even, velocity odd.
pub const WORDS: usize = 4;

fn word(state: State, phase: u32) -> u32 {
    state.encode() | (phase & PHASE_MASK) << STATE_BITS
}

/// The world: the lattice's cells, four `u32` words each (see `WORDS`), cell by cell, row by row,
/// and the physics distribution's tunables.
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

    /// The census of what is conserved: the substantial particles, in `Material::ALL` order, with
    /// zero for vacuum and air (air is insubstantial: it can cease to exist and spawn anew).
    pub fn conserved(&self) -> [usize; 6] {
        Material::ALL.map(|m| if m.is_substantial() { self.count(m) } else { 0 })
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

    /// Overwrites the cell's current words, keeping its phase.
    pub(crate) fn set(&mut self, x: i32, y: i32, s: State) {
        let i = self.index(x, y);
        let parity = self.current(i);
        let slot = WORDS * i + parity;
        self.cells[slot] = word(s, self.cells[slot] >> STATE_BITS);
        self.cells[WORDS * i + 2 + parity] = s.velocity.encode();
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
        let before = self.conserved();
        for y in 0..self.height {
            for x in 0..self.width {
                self.update(x, y);
            }
        }
        #[cfg(debug_assertions)]
        assert_eq!(
            before,
            self.conserved(),
            "substantial particles not conserved by a step at phase {}",
            self.phase(0, 0)
        );
    }
}
