//! The CPU reference simulation. Every rule lives here first, in plain Rust,
//! and is tested natively; the WGSL compute shaders mirror it bit for bit.
//!
//! Names follow the Primitives section of the Falling Sand Engine tab:
//! a column-offset hexagonal lattice of cells, each holding sand, water, slate, bedrock or air,
//! advanced by synchronizer-phasing, moving matter by a directed handshake
//! (offer → accept → transfer) at reach 1 with self-writes only.

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

/// Left or right: the horizontal half of a move, and the side water spreads toward.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[repr(u8)]
pub enum Side {
    Left = 0,
    Right = 1,
}

impl Side {
    pub fn flip(self) -> Side {
        match self {
            Side::Left => Side::Right,
            Side::Right => Side::Left,
        }
    }

    /// The side a move toward `d` goes to, if it goes to one.
    pub fn of(d: Dir) -> Option<Side> {
        match d {
            Dir::UpLeft | Dir::DownLeft => Some(Side::Left),
            Dir::UpRight | Dir::DownRight => Some(Side::Right),
            Dir::Up | Dir::Down => None,
        }
    }

    /// The upper diagonal on this side: where a side move goes.
    pub fn up(self) -> Dir {
        match self {
            Side::Left => Dir::UpLeft,
            Side::Right => Dir::UpRight,
        }
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
const SALT_FLOW: u32 = 3;

// Cell state

/// The discriminants are the encoded values, shared with the shaders.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[repr(u8)]
pub enum Material {
    /// Not matter: what a cell holds when it holds no matter.
    Air = 0,
    Sand = 1,
    /// A fluid: falls like sand, and where it can't fall, steps to a side.
    Water = 2,
    /// Solid matter that never moves.
    Slate = 3,
    /// Slate that can't be erased. Outside the lattice is bedrock.
    Bedrock = 4,
}

/// What a cell holds this phase.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct State {
    pub material: Material,
    /// The direction this cell is sending its matter to.
    pub offer: Option<Dir>,
    /// The direction this cell is receiving matter from.
    pub accept: Option<Dir>,
    /// Water only: the side it is spreading toward.
    pub flow: Side,
    /// Water only: its side offer has already gone a phase unanswered.
    pub waited: bool,
    /// Sand only: it holds a packet of the water it displaced, for one phase, offered back to
    /// the cell the sand came from. `flow` is the packet's.
    pub carries: bool,
}

impl State {
    pub const AIR: State =
        State { material: Material::Air, offer: None, accept: None, flow: Side::Left, waited: false, carries: false };
    pub const SLATE: State = State { material: Material::Slate, ..State::AIR };
    pub const BEDROCK: State = State { material: Material::Bedrock, ..State::AIR };

    pub fn holding(material: Material) -> State {
        State { material, ..State::AIR }
    }

    /// 9 bits: material (bits 0-2: 0 air, 1 sand, 2 water, 3 slate, 4 bedrock), then one intent
    /// (bits 3-5), stored as 1 + a direction's index; then the flow (bit 6, set for right: water's
    /// own, or the packet's for sand carrying one); then water's waited or sand's carries (bit 7);
    /// then whether the intent is an acceptance (bit 8). A cell holds at most one intent: sand's
    /// offer, water's offer or acceptance, or air's acceptance.
    pub fn encode(self) -> u32 {
        let dir = |d: Option<Dir>| d.map_or(0, |d| d as u32 + 1);
        let extra = match self.material {
            Material::Water => (self.flow as u32) << 6 | (self.waited as u32) << 7,
            Material::Sand if self.carries => (self.flow as u32) << 6 | 1 << 7,
            _ => 0,
        };
        let accepting = (self.accept.is_some() as u32) << 8;
        self.material as u32 | dir(self.offer.or(self.accept)) << 3 | extra | accepting
    }

    pub fn decode(bits: u32) -> State {
        let v = bits >> 3 & 7;
        let intent = (v != 0).then(|| Dir::ALL[(v - 1) as usize % 6]);
        let (offer, accept) = if bits >> 8 & 1 == 1 { (None, intent) } else { (intent, None) };
        let flow = if bits >> 6 & 1 == 1 { Side::Right } else { Side::Left };
        let flag = bits >> 7 & 1 == 1;
        match bits & 7 {
            0 => State { accept: intent, ..State::AIR },
            1 => State {
                material: Material::Sand,
                offer,
                carries: flag,
                flow: if flag { flow } else { Side::Left },
                ..State::AIR
            },
            2 => State { material: Material::Water, offer, accept, flow, waited: flag, ..State::AIR },
            3 => State::SLATE,
            _ => State::BEDROCK,
        }
    }

    /// The matter an offer from this cell sends: the packet, if it carries one.
    pub fn sends(self) -> Material {
        if self.carries { Material::Water } else { self.material }
    }
}

// The rule

/// One cell's update: its next state from its own state and what it sees around it, all at
/// the same phase (outside the lattice reads as bedrock). Every cell computes only itself, so
/// the two halves of a transfer agree because they read the same snapshot.
pub fn next(me: State, seen: [State; 6], x: i32, y: i32, phase: u32) -> State {
    let at = |d: Dir| seen[d as usize];
    let mut s = me;

    // Transfer, the giving half: my offer was accepted, so I hand over what I sent.
    if let Some(d) = me.offer
        && at(d).accept == Some(d.opposite())
    {
        if me.carries {
            // The packet has gone; plain sand is left.
            s = State::holding(Material::Sand);
        } else if at(d).material == Material::Air {
            return State::AIR;
        } else {
            // Displacement: the receiver held lighter matter, which comes back to me next phase. I
            // hold air already accepting it, so nothing else falls into the gap.
            return State { accept: Some(d), ..State::AIR };
        }
    }

    // Transfer, the receiving half: the offer I accepted still stands, so I take what it sends.
    // Water brings its flow, turned toward the side it moved to.
    if let Some(d) = me.accept {
        s.accept = None;
        let n = at(d);
        if n.offer == Some(d.opposite()) {
            let sent = n.sends();
            if me.material == Material::Air {
                let flow = if sent == Material::Water { Side::of(d.opposite()).unwrap_or(n.flow) } else { Side::Left };
                s = State { material: sent, flow, ..State::AIR };
            } else {
                // Displacement: I take the heavier matter and carry what I held as a packet, offered
                // straight back to where the matter came from.
                s = State { material: sent, carries: true, flow: me.flow, offer: Some(d), ..State::AIR };
            }
        }
    }

    // A neighbor `m` could offer to: it holds lighter matter (or air), isn't falling and hasn't
    // accepted anything. Water stepping aside still counts: it takes heavier matter first.
    let unmoved = |n: State| n.offer.is_none_or(|e| matches!(e, Dir::UpLeft | Dir::UpRight));
    let free = |m: Material, d: Dir| {
        let n = at(d);
        lighter(n.material, m) && unmoved(n) && n.accept.is_none()
    };
    // An offer stands while its target still could take it and hasn't accepted someone else.
    let standing = |m: Material, d: Dir| {
        let n = at(d);
        lighter(n.material, m) && unmoved(n) && (n.accept.is_none() || n.accept == Some(d.opposite()))
    };

    match s.material {
        Material::Air => {
            if me.accept.is_none() {
                // Accept: the steepest offer aimed at me wins; ties are hashed. A side offer, from
                // below, is taken only where I could pass the water on downhill, so water never climbs.
                // A packet is only ever offered to its own gap, which has already accepted it.
                let passes_on = |from: Dir| {
                    let far = match from {
                        Dir::DownLeft => Dir::DownRight,
                        Dir::DownRight => Dir::DownLeft,
                        _ => return false,
                    };
                    at(Dir::Down).material == Material::Air || at(far).material == Material::Air
                };
                let offers = Dir::ALL.into_iter().filter(|&d| {
                    let n = at(d);
                    matches!(n.material, Material::Sand | Material::Water)
                        && !n.carries
                        && n.offer == Some(d.opposite())
                        && (drop(x, y, d) < 0 || passes_on(d))
                });
                s.accept = pick(offers, |d| -drop(x, y, d), hash(x, y, phase, SALT_ACCEPT));
            }
        }
        Material::Sand if s.carries => {
            // My packet is offered back to the gap I came from, which already accepts it and which
            // the brush leaves alone, so it always goes next phase.
        }
        Material::Sand => {
            if s.offer.is_some_and(|d| !standing(Material::Sand, d)) {
                s.offer = None;
            }
            if s.offer.is_none() {
                // Offer: the steepest downhill neighbor free to take sand (air, or still water); ties
                // are hashed.
                let open = Dir::ALL.into_iter().filter(|&d| drop(x, y, d) > 0 && free(Material::Sand, d));
                s.offer = pick(open, |d| drop(x, y, d), hash(x, y, phase, SALT_OFFER));
            }
        }
        Material::Water => {
            let free = |d: Dir| free(Material::Water, d);
            let standing = |d: Dir| standing(Material::Water, d);
            if let Some(d) = s.offer {
                if !standing(d) {
                    s.offer = None;
                } else if drop(x, y, d) < 0 && at(d).accept.is_none() {
                    // A side offer. The air reads it a phase after it is made, so no answer a
                    // second time means declined: flip, and try the other side.
                    if s.waited {
                        s.offer = None;
                        s.flow = s.flow.flip();
                    } else {
                        s.waited = true;
                    }
                }
            }
            if s.offer.is_none() {
                s.waited = false;
                // Fall: straight down first, then the diagonal on the flow side.
                let flow = s.flow;
                let open = Dir::ALL.into_iter().filter(|&d| drop(x, y, d) > 0 && free(d));
                s.offer = pick(open, |d| 2 * drop(x, y, d) + (Side::of(d) == Some(flow)) as i32, 0);
                if s.offer.is_none() {
                    // Side step: the flow side first, else the other, which becomes the flow.
                    if let Some(side) = [flow, flow.flip()].into_iter().find(|side| free(side.up())) {
                        s.offer = Some(side.up());
                        s.flow = side;
                    }
                }
            }
            if s.offer.is_none_or(|d| drop(x, y, d) < 0) && me.accept.is_none() {
                // Displacement: water that can't fall accepts heavier matter offered from above,
                // rather than step aside; the steepest offer first, ties hashed.
                let offers = Dir::ALL.into_iter().filter(|&d| {
                    let n = at(d);
                    drop(x, y, d) < 0 && n.offer == Some(d.opposite()) && lighter(Material::Water, n.sends())
                });
                if let Some(d) = pick(offers, |d| -drop(x, y, d), hash(x, y, phase, SALT_ACCEPT)) {
                    s.accept = Some(d);
                    s.offer = None;
                    s.waited = false;
                }
            }
        }
        Material::Slate | Material::Bedrock => {}
    }
    s
}

/// How heavy a material is: matter sinks through lighter matter, and air is lightest. Slate and
/// bedrock never move, so they have none.
pub fn density(m: Material) -> Option<u8> {
    match m {
        Material::Air => Some(0),
        Material::Water => Some(1),
        Material::Sand => Some(2),
        Material::Slate | Material::Bedrock => None,
    }
}

/// Whether `a` is lighter than `b`, so `b` can sink into it.
pub fn lighter(a: Material, b: Material) -> bool {
    matches!((density(a), density(b)), (Some(a), Some(b)) if a < b)
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
const STATE_BITS: u32 = 9;
const STATE_MASK: u32 = (1 << STATE_BITS) - 1;
const PHASE_SHIFT: u32 = 2 * STATE_BITS;
/// The phase is stored in the remaining 14 bits and wraps; neighbors are never more than one apart.
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

    /// What the cell holds once any packet in flight has landed: a sinking grain's gap holds the
    /// water it is about to get back. A world drawn this way holds every packet's water.
    pub fn landed(&self, x: i32, y: i32) -> Material {
        let s = self.cell(x, y);
        let packet = s.accept.is_some_and(|d| {
            let (nx, ny) = neighbor(x, y, d);
            self.in_bounds(nx, ny) && self.cell(nx, ny).carries
        });
        if s.material == Material::Air && packet { Material::Water } else { s.material }
    }

    /// How many cells' worth of `material` the world holds, packets included.
    pub fn count(&self, material: Material) -> usize {
        let held = |s: State| (s.material == material) as usize + (s.sends() == material && s.carries) as usize;
        self.cells.iter().map(|&w| held(State::decode(w & STATE_MASK))).sum()
    }

    /// The brush, the one source and sink of matter: the cell now holds `material` and nothing
    /// in flight. Matter goes only into a cell holding air; erasing (painting air) clears any
    /// matter but bedrock. Air that has accepted matter is left alone. Call it between phases.
    pub fn paint(&mut self, x: i32, y: i32, material: Material) {
        let here = self.cell(x, y);
        // Air that has accepted matter is left alone, painting or erasing: the matter arrives next
        // phase. Above all, a gap a sinking grain left is where its packet of water must go.
        let allowed = match (material, here.material) {
            (_, Material::Air) => here.accept.is_none() && material != Material::Air,
            (Material::Air, m) => m != Material::Bedrock,
            _ => false,
        };
        if !allowed {
            return;
        }
        let i = self.index(x, y);
        let mut s = State::holding(material);
        if material == Material::Water && hash(x, y, 0, SALT_FLOW) & 1 == 1 {
            s.flow = Side::Right;
        }
        self.cells[i] = pack(s, s, self.cells[i] >> PHASE_SHIFT);
    }

    /// Sets a cell's state outright, for tests that need a particular flow.
    #[cfg(test)]
    fn put(&mut self, x: i32, y: i32, s: State) {
        let i = self.index(x, y);
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
