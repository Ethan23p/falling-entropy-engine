//! The CPU reference simulation. Every rule lives here first, in plain Rust,
//! and is tested natively; the WGSL compute shaders mirror it bit for bit.
//!
//! Names follow the Primitives section of the Falling Sand Engine tab:
//! a column-offset hexagonal lattice of cells, each holding sand, water, slate, bedrock or air,
//! advanced by synchronizer-phasing, moving matter by directed, conditional contracts at reach 1
//! with self-writes only. An acceptance is receipt: the receiver writes itself holding the matter
//! and names its giver, and the giver clears when it reads that.

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
const SALT_START: u32 = 4;
const SALT_EARN: u32 = 5;
const SALT_FRICTION: u32 = 6;

/// Whether random `bits` fall within `chance`, out of 65,536.
fn within(bits: u32, chance: u32) -> bool {
    bits & 0xffff < chance
}

// Motion

/// The top speed. A cell at it offers as soon as it is free, so it falls a cell every phase: the
/// lattice's speed of light, the pace at which a contract's news travels. Sand and water build up
/// to it; a fast material could start there.
pub const TOP_SPEED: u8 = 3;

/// The feel of motion, tuned in the config panel. Chances are out of 65,536, worked out here once
/// so that the shaders, handed the same integers, decide exactly as the reference does.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Rules {
    /// The chance a cell free to fall offers this phase, by its speed: from `start` at rest to
    /// always at top speed.
    pub start: [u32; 4],
    /// The chance a straight fall earns a level of speed. A diagonal fall's is half.
    pub gravity: u32,
    /// The most speed water keeps when it lands.
    pub splash: u8,
    /// The chance a level side step that isn't pushed costs a level of speed.
    pub friction: u32,
}

impl Rules {
    /// The shipped values: the config panel's defaults.
    pub const START: f32 = 0.25;
    pub const GRAVITY: f32 = 0.5;
    pub const SPLASH: u8 = 2;
    pub const FRICTION: f32 = 0.1;

    pub fn new(start: f32, gravity: f32, splash: u8, friction: f32) -> Rules {
        let chance = |p: f32| (p.clamp(0.0, 1.0) * 65536.0).round() as u32;
        let start = [0, 1, 2, 3].map(|e| chance(start + (1.0 - start) * e as f32 / TOP_SPEED as f32));
        Rules { start, gravity: chance(gravity), splash: splash.min(TOP_SPEED), friction: chance(friction) }
    }
}

impl Default for Rules {
    fn default() -> Rules {
        Rules::new(Rules::START, Rules::GRAVITY, Rules::SPLASH, Rules::FRICTION)
    }
}

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

/// What a cell publishes this phase: what it holds, and its side of any contract.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct State {
    pub material: Material,
    /// The neighbor this cell offers its matter to (its packet, if it carries one).
    pub offer: Option<Dir>,
    /// The neighbor this cell took matter from in this update: receipt, naming its giver.
    pub accept: Option<Dir>,
    /// Water only: the side it is spreading toward.
    pub flow: Side,
    /// Water only: it offered a side step last phase and is holding for the answer; if the
    /// answer is no, it flips.
    pub waited: bool,
    /// Sand only: it holds a packet of the water it displaced, offered back to the cell the sand
    /// came from. `flow` is the packet's.
    pub carries: bool,
    /// Sand and water: how fast it is moving, 0 to `TOP_SPEED`.
    pub speed: u8,
}

impl State {
    pub const AIR: State = State {
        material: Material::Air,
        offer: None,
        accept: None,
        flow: Side::Left,
        waited: false,
        carries: false,
        speed: 0,
    };
    pub const SLATE: State = State { material: Material::Slate, ..State::AIR };
    pub const BEDROCK: State = State { material: Material::Bedrock, ..State::AIR };

    pub fn holding(material: Material) -> State {
        State { material, ..State::AIR }
    }

    /// 13 bits: material (bits 0-2: 0 air, 1 sand, 2 water, 3 slate, 4 bedrock); the offer (bits
    /// 3-5) and the acceptance (bits 6-8), each stored as 1 + a direction's index, 0 for none; the
    /// flow (bit 9, set for right: water's own, or the packet's for sand carrying one); water's
    /// waited or sand's carries (bit 10); sand's or water's speed (bits 11-12). Only sand and water
    /// hold anything but a material, except air the brush just erased a receiver to, which keeps the
    /// acceptance for a phase.
    pub fn encode(self) -> u32 {
        let dir = |d: Option<Dir>| d.map_or(0, |d| d as u32 + 1);
        match self.material {
            Material::Air => return dir(self.accept) << 6,
            Material::Slate | Material::Bedrock => return self.material as u32,
            Material::Sand | Material::Water => {}
        }
        let (flow, flag) = match self.material {
            Material::Water => (self.flow as u32, self.waited as u32),
            _ if self.carries => (self.flow as u32, 1),
            _ => (0, 0),
        };
        self.material as u32
            | dir(self.offer) << 3
            | dir(self.accept) << 6
            | flow << 9
            | flag << 10
            | (self.speed.min(TOP_SPEED) as u32) << 11
    }

    pub fn decode(bits: u32) -> State {
        let dir = |v: u32| (v != 0).then(|| Dir::ALL[(v - 1) as usize % 6]);
        let material = match bits & 7 {
            0 => return State { accept: dir(bits >> 6 & 7), ..State::AIR },
            1 => Material::Sand,
            2 => Material::Water,
            3 => return State::SLATE,
            _ => return State::BEDROCK,
        };
        let flag = bits >> 10 & 1 == 1;
        let flow = if bits >> 9 & 1 == 1 { Side::Right } else { Side::Left };
        State {
            material,
            offer: dir(bits >> 3 & 7),
            accept: dir(bits >> 6 & 7),
            flow: if material == Material::Water || flag { flow } else { Side::Left },
            waited: material == Material::Water && flag,
            carries: material == Material::Sand && flag,
            speed: (bits >> 11 & 3) as u8,
        }
    }

    /// The matter an offer from this cell sends: the packet, if it carries one.
    pub fn sends(self) -> Material {
        if self.carries { Material::Water } else { self.material }
    }

    /// The matter this cell took in its acceptance: sand, if it took sand into water and now
    /// carries the water as a packet.
    pub fn received(self) -> Material {
        if self.carries { Material::Sand } else { self.material }
    }
}

// The rule

/// One cell's update: its next state from its own state and what it sees around it, all at
/// the same phase (outside the lattice reads as bedrock). Every cell computes only itself, and
/// every party to a contract derives its own outcome from what has reached it:
///
/// - An offer stands while it is repeated. The phase after offering, a cell can't yet know the
///   answer, so it may only repeat the offer or hold, and accepts nothing.
/// - An acceptance is receipt: the receiver holds the matter from this update on and names its
///   giver. It may offer the matter onward in the same update. It never takes the same giver two
///   phases running, since the repeat of an offer it took is a record, not more matter.
/// - The giver reads the acceptance naming it on its next update and clears: its copy was a
///   record of a finished send.
pub fn next(me: State, seen: [State; 6], x: i32, y: i32, phase: u32, rules: &Rules) -> State {
    let at = |d: Dir| seen[d as usize];

    // A neighbor `m` could offer to: one holding lighter matter that isn't falling (air, or water
    // still or stepping aside), or one below it falling straight down, which it takes the moment
    // its own matter is gone (a follow). The cell below is claimed, not open, while a neighbor
    // beside it is falling into it diagonally: it fills as I would move, so a mass falling
    // together moves on diagonally rather than wait on a straight fall it can't make.
    let open = |m: Material, d: Dir| {
        let n = at(d);
        let still = n.offer.is_none_or(|e| matches!(e, Dir::UpLeft | Dir::UpRight));
        let follow = d == Dir::Down && moves(n.material) && !n.carries && n.offer == Some(Dir::Down);
        let into = |side: Dir, e: Dir| moves(at(side).material) && at(side).offer == Some(e);
        let claimed = d == Dir::Down && (into(Dir::DownLeft, Dir::DownRight) || into(Dir::DownRight, Dir::DownLeft));
        (lighter(n.material, m) && still && !claimed) || follow
    };

    // Clear: a neighbor's acceptance names me, so what I offered is theirs. The packet, if I
    // carried one, else everything I held.
    let taken = Dir::ALL.into_iter().any(|d| at(d).accept == Some(d.opposite()));
    let mut s = State { accept: None, ..me };
    if taken {
        s = if me.carries { State::holding(Material::Sand) } else { State::AIR };
    } else if let Some(d) = me.offer {
        // Pending: the answer to my last offer isn't in. A packet's gap always takes it, a fall
        // stands while its target could still take it, and a side step is offered once. Holding
        // withdraws the offer: next phase the answer is in and the matter is free.
        let repeat = me.carries || (drop(x, y, d) > 0 && open(me.material, d));
        s.offer = repeat.then_some(d);
        s.waited = me.material == Material::Water && drop(x, y, d) < 0;
        return s;
    }

    // Accept, into a cell holding nothing: a packet coming back first, then the steepest offer;
    // ties are hashed. A side offer, from below, is taken only where I could pass the water on
    // downhill, so water never climbs.
    if s.material == Material::Air {
        let passes_on = |from: Dir| {
            let far = match from {
                Dir::DownLeft => Dir::DownRight,
                Dir::DownRight => Dir::DownLeft,
                _ => return false,
            };
            at(Dir::Down).material == Material::Air || at(far).material == Material::Air
        };
        let offered = |d: Dir| {
            let n = at(d);
            moves(n.material) && n.offer == Some(d.opposite()) && me.accept != Some(d)
        };
        let packet = Dir::ALL.into_iter().find(|&d| offered(d) && at(d).carries);
        let fall = || {
            let offers = Dir::ALL.into_iter().filter(|&d| offered(d) && (drop(x, y, d) < 0 || passes_on(d)));
            pick(offers, |d| -drop(x, y, d), hash(x, y, phase, SALT_ACCEPT))
        };
        if let Some(d) = packet.or_else(fall) {
            let n = at(d);
            // Speed travels with the matter, plus what a fall earned: a level on the gravity
            // chance, half that for a diagonal. A side step was paid for when offered. A packet
            // arrives still. Water brings its flow, turned toward the side it moved to.
            let speed = if n.carries {
                0
            } else if drop(x, y, d) < 0 {
                let chance = if d == Dir::Up { rules.gravity } else { rules.gravity / 2 };
                let earned = within(hash(x, y, phase, SALT_EARN), chance) as u8;
                (n.speed + earned).min(TOP_SPEED)
            } else {
                n.speed
            };
            let flow = if n.carries { n.flow } else { Side::of(d.opposite()).unwrap_or(n.flow) };
            let flow = if n.sends() == Material::Water { flow } else { Side::Left };
            s = State { material: n.sends(), accept: Some(d), flow, speed, ..State::AIR };
        }
    }

    // Start: a free fall straight down into air goes on a chance that grows with the speed, so
    // matter starts slow and builds up. Slides, follows and sinking don't wait.
    let start = |d: Dir, speed: u8| {
        let free_fall = d == Dir::Down && at(d).material == Material::Air;
        (!free_fall || within(hash(x, y, phase, SALT_START), rules.start[speed as usize])).then_some(d)
    };

    match s.material {
        Material::Sand if s.carries => {}
        Material::Sand => {
            // Offer: the steepest downhill neighbor open to sand; ties are hashed. Nowhere to go:
            // landed, and sand stops dead.
            let open = Dir::ALL.into_iter().filter(|&d| drop(x, y, d) > 0 && open(Material::Sand, d));
            match pick(open, |d| drop(x, y, d), hash(x, y, phase, SALT_OFFER)) {
                Some(d) => s.offer = start(d, s.speed),
                None => s.speed = 0,
            }
        }
        Material::Water => {
            // Declined: my side step went unanswered, so I flip and try the other way.
            if s.waited {
                s.waited = false;
                s.flow = s.flow.flip();
            }
            // Fall: straight down first, then the diagonal on the flow side.
            let flow = s.flow;
            let falls = Dir::ALL.into_iter().filter(|&d| drop(x, y, d) > 0 && open(Material::Water, d));
            let fall = pick(falls, |d| 2 * drop(x, y, d) + (Side::of(d) == Some(flow)) as i32, 0);
            // Trade: water that can't fall takes heavier matter offered from above rather than
            // step aside; the steepest offer first, ties hashed. It holds the sand and carries its
            // own water as a packet, offered straight back to the giver, which takes it on its
            // next update as it clears: no air ever opens between them.
            let heavier = Dir::ALL.into_iter().filter(|&d| {
                let n = at(d);
                drop(x, y, d) < 0 && n.offer == Some(d.opposite()) && lighter(Material::Water, n.sends())
            });
            let trade = if fall.is_none() && s.accept.is_none() {
                pick(heavier, |d| -drop(x, y, d), hash(x, y, phase, SALT_ACCEPT))
            } else {
                None
            };
            if let Some(d) = fall {
                // About to fall, so not stepping aside.
                s.offer = start(d, s.speed);
            } else if let Some(d) = trade {
                s = State { material: at(d).sends(), accept: Some(d), offer: Some(d), carries: true, ..s };
                s.speed = 0;
            } else {
                // Landed: water keeps at most the splash. Side step: the flow side first, else the
                // other, which becomes the flow. Pushed, by matter above it or above it on the side
                // it leaves, the step is free; otherwise it costs a level of speed, paid as the
                // offer is made, and with none left the water rests.
                s.speed = s.speed.min(rules.splash);
                // Matter moving into me (a record of what I just took) doesn't push.
                let pushes = |d: Dir| moves(at(d).material) && at(d).offer != Some(d.opposite());
                let pushed = |side: Side| pushes(Dir::Up) || pushes(side.flip().up());
                let speed = s.speed;
                let step = |side: &Side| open(Material::Water, side.up()) && (speed > 0 || pushed(*side));
                if let Some(side) = [flow, flow.flip()].into_iter().find(step) {
                    s.offer = Some(side.up());
                    s.flow = side;
                    if !pushed(side) && within(hash(x, y, phase, SALT_FRICTION), rules.friction) {
                        s.speed -= 1;
                    }
                }
            }
        }
        Material::Air | Material::Slate | Material::Bedrock => {}
    }
    s
}

/// Whether `m` is matter that moves: sand or water.
pub fn moves(m: Material) -> bool {
    matches!(m, Material::Sand | Material::Water)
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

/// A cell keeps two words, one per phase parity: the state it published at a phase goes in the
/// word of that phase's parity, tagged with the phase. A neighbor at phase `p` reads the word of
/// `p`'s parity, which the cell doesn't overwrite until it reaches `p + 2`, so a neighbor one phase
/// ahead still serves it (the kept-back value), and no reader needs another word in step with it.
const STATE_BITS: u32 = 16;
const STATE_MASK: u32 = (1 << STATE_BITS) - 1;
/// The phase is stored in the top 16 bits and wraps; neighbors are never more than one apart.
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
    /// The feel of motion; the shipped values unless changed.
    pub rules: Rules,
}

impl World {
    pub fn new(width: i32, height: i32) -> World {
        // Phase 0 in the even word; the odd word holds phase -1.
        let pair = [word(State::AIR, 0), word(State::AIR, PHASE_MASK)];
        World { width, height, cells: pair.repeat((width * height) as usize), rules: Rules::default() }
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

    pub fn material(&self, x: i32, y: i32) -> Material {
        self.cell(x, y).material
    }

    pub fn phase(&self, x: i32, y: i32) -> u32 {
        self.cells[self.current(self.index(x, y))] >> STATE_BITS
    }

    /// The neighbor that took what this cell offered: its acceptance names this cell. Then the
    /// cell's copy is a record, and the matter (or the packet, if it carries one) is the taker's.
    pub fn taker(&self, x: i32, y: i32) -> Option<Dir> {
        Dir::ALL.into_iter().find(|&d| {
            let (nx, ny) = neighbor(x, y, d);
            self.in_bounds(nx, ny) && self.cell(nx, ny).accept == Some(d.opposite())
        })
    }

    /// What the observer sees: what the cell owns, with a record drawn as what replaces it. A
    /// giver whose matter was taken reads as air, or as water where the packet of a grain that
    /// sank from it is on its way back; sand carrying a packet reads as sand. A world drawn this
    /// way holds every unit of matter exactly once.
    pub fn looks(&self, x: i32, y: i32) -> Material {
        let s = self.cell(x, y);
        match self.taker(x, y) {
            Some(_) if s.carries => Material::Sand,
            Some(d) => {
                let (nx, ny) = neighbor(x, y, d);
                let t = self.cell(nx, ny);
                if t.carries && t.offer == Some(d.opposite()) { Material::Water } else { Material::Air }
            }
            None => s.material,
        }
    }

    /// How many cells' worth of `material` the world owns: packets included, records not.
    pub fn count(&self, material: Material) -> usize {
        let mut n = 0;
        for y in 0..self.height {
            for x in 0..self.width {
                let s = self.cell(x, y);
                let taken = self.taker(x, y).is_some();
                n += (s.received() == material && !(taken && !s.carries)) as usize;
                n += (s.carries && !taken && material == Material::Water) as usize;
            }
        }
        n
    }

    /// Whether every cell is at the same phase, as after a whole `step`.
    pub fn uniform(&self) -> bool {
        let p = self.phase(0, 0);
        (0..self.height).all(|y| (0..self.width).all(|x| self.phase(x, y) == p))
    }

    /// The contract rules, checked on a world whose cells share a phase: every offer comes from a
    /// cell holding moving matter; every acceptance names a giver still holding a record of what
    /// was taken, offering it nowhere else; no giver is named twice. So every unit of matter has
    /// exactly one owner.
    pub fn audit(&self) -> Result<(), String> {
        let fail = |x: i32, y: i32, why: &str| Err(format!("({x}, {y}): {why}\n{}", self.ascii_intents()));
        for y in 0..self.height {
            for x in 0..self.width {
                let s = self.cell(x, y);
                if (s.offer.is_some() || s.accept.is_some()) && !moves(s.material) {
                    return fail(x, y, "an offer or acceptance without matter");
                }
                let takers = Dir::ALL
                    .into_iter()
                    .filter(|&d| {
                        let (nx, ny) = neighbor(x, y, d);
                        self.in_bounds(nx, ny) && self.cell(nx, ny).accept == Some(d.opposite())
                    })
                    .count();
                if takers > 1 {
                    return fail(x, y, "taken by two neighbors");
                }
                let Some(d) = s.accept else { continue };
                let (gx, gy) = neighbor(x, y, d);
                if !self.in_bounds(gx, gy) {
                    return fail(x, y, "accepted from outside the lattice");
                }
                let g = self.cell(gx, gy);
                if g.sends() != s.received() {
                    return fail(x, y, "the giver holds no record of what was taken");
                }
                if g.offer.is_some_and(|e| e != d.opposite()) {
                    return fail(x, y, "the giver offers what was taken elsewhere");
                }
            }
        }
        Ok(())
    }

    /// The brush, the one source and sink of matter: the cell now holds `material` and nothing
    /// in flight. Matter goes only into a cell holding air; erasing (painting air) clears any
    /// matter but bedrock. It honors open contracts: a record (a giver whose matter was just taken)
    /// is left alone, and erasing a receiver leaves air still naming its giver for a phase, so the
    /// giver clears rather than take the matter back. Call it between phases.
    pub fn paint(&mut self, x: i32, y: i32, material: Material) {
        let here = self.cell(x, y);
        let allowed = match (material, here.material) {
            (Material::Air, m) => m != Material::Air && m != Material::Bedrock,
            (_, m) => m == Material::Air && here.accept.is_none(),
        };
        if !allowed || self.taker(x, y).is_some() {
            return;
        }
        // Erasing a receiver leaves its acceptance standing for a phase, on air: its giver reads it
        // and clears, as if the matter had stayed. Nothing else moves, so every cell's acceptance is
        // the same before and after the brush, and each cell decides its own paint.
        let mut s = State { accept: here.accept, ..State::holding(material) };
        if material == Material::Water && hash(x, y, 0, SALT_FLOW) & 1 == 1 {
            s.flow = Side::Right;
        }
        self.set(x, y, s);
    }

    /// Overwrites the cell's current word, keeping its phase.
    fn set(&mut self, x: i32, y: i32, s: State) {
        let i = self.current(self.index(x, y));
        self.cells[i] = word(s, self.cells[i] >> STATE_BITS);
    }

    /// Sets a cell's state outright, for tests that need a particular flow or contract.
    #[cfg(test)]
    fn put(&mut self, x: i32, y: i32, s: State) {
        self.set(x, y, s);
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
    /// neighbor's word of its own phase's parity. Returns whether it advanced.
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
        let ahead = phase.wrapping_add(1) & PHASE_MASK;
        self.cells[2 * i + (ahead & 1) as usize] = word(next(me, seen, x, y, phase, &self.rules), ahead);
        true
    }

    /// One phase for every cell, row by row. Any order gives the same result. Debug builds audit
    /// the contract rules after every step.
    pub fn step(&mut self) {
        for y in 0..self.height {
            for x in 0..self.width {
                self.update(x, y);
            }
        }
        #[cfg(debug_assertions)]
        if let Err(e) = self.audit() {
            panic!("contract broken after a step at phase {}: {e}", self.phase(0, 0));
        }
    }
}
