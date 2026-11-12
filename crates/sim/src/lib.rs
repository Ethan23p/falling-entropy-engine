//! The CPU reference simulation. Every rule lives here first, in plain Rust,
//! and is tested natively; the WGSL compute shaders mirror it bit for bit.
//!
//! Names follow the Primitives section of the Falling Sand Engine tab:
//! a column-offset hexagonal lattice of cells, each holding sand, water, slate, bedrock or air,
//! advanced by synchronizer-phasing, moving matter by directed, conditional contracts at reach 1
//! with self-writes only. Every offer is answered, accepted or rejected, on the target's next
//! update. An acceptance is receipt: the receiver writes itself holding the matter and names its
//! giver, and the giver clears, to what the offer's terms send back, when it reads that.

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
    /// The neighbor this cell offers what it holds to: a directed, conditional offer, answered on
    /// the neighbor's next update.
    pub offer: Option<Dir>,
    /// The offer's terms: what comes back to this cell if it is accepted. Air, or water for a trade.
    pub terms: Material,
    /// The neighbor whose offer this cell accepted in this update: receipt, naming its giver.
    pub accept: Option<Dir>,
    /// The neighbors whose offers this cell turned down in this update, a bit per direction.
    pub rejects: u8,
    /// Sand and water: how fast it is moving, 0 to `TOP_SPEED`.
    pub speed: u8,
    /// Water only: the side it is spreading toward.
    pub flow: Side,
}

impl State {
    pub const AIR: State = State {
        material: Material::Air,
        offer: None,
        terms: Material::Air,
        accept: None,
        rejects: 0,
        speed: 0,
        flow: Side::Left,
    };
    pub const SLATE: State = State { material: Material::Slate, ..State::AIR };
    pub const BEDROCK: State = State { material: Material::Bedrock, ..State::AIR };

    pub fn holding(material: Material) -> State {
        State { material, ..State::AIR }
    }

    /// Whether this cell turned down an offer from `d`.
    pub fn rejected(self, d: Dir) -> bool {
        self.rejects >> d as u8 & 1 == 1
    }

    /// 19 bits: contents (bits 0-2: 0 air, 1 sand, 2 water, 3 slate, 4 bedrock); the offer's
    /// direction (bits 3-5, 1 + a direction's index, 0 for none) and terms (bit 6, set when water
    /// comes back); the acceptance's direction (bits 7-9, as the offer's); the rejections (bits
    /// 10-15, bit 10 + d for an offer from direction d); speed (bits 16-17); flow (bit 18, set for
    /// right).
    pub fn encode(self) -> u32 {
        let dir = |d: Option<Dir>| d.map_or(0, |d| d as u32 + 1);
        self.material as u32
            | dir(self.offer) << 3
            | ((self.terms == Material::Water) as u32) << 6
            | dir(self.accept) << 7
            | (self.rejects as u32 & 63) << 10
            | (self.speed.min(TOP_SPEED) as u32) << 16
            | (self.flow as u32) << 18
    }

    pub fn decode(bits: u32) -> State {
        let dir = |v: u32| (v != 0).then(|| Dir::ALL[(v - 1) as usize % 6]);
        let material = match bits & 7 {
            0 => Material::Air,
            1 => Material::Sand,
            2 => Material::Water,
            3 => Material::Slate,
            _ => Material::Bedrock,
        };
        State {
            material,
            offer: dir(bits >> 3 & 7),
            terms: if bits >> 6 & 1 == 1 { Material::Water } else { Material::Air },
            accept: dir(bits >> 7 & 7),
            rejects: (bits >> 10 & 63) as u8,
            speed: (bits >> 16 & 3) as u8,
            flow: if bits >> 18 & 1 == 1 { Side::Right } else { Side::Left },
        }
    }
}

// The rule

/// One cell's update: its next state from its own state, the state it kept back from the phase
/// before, and what it sees around it, all at the same phase (outside the lattice reads as
/// bedrock). Every cell computes only itself, in the three steps of the tab's Contracts:
///
/// 1. Clear. The offer it made last phase, kept back in its older word, has its answer now.
///    Accepted, the matter is the receiver's, and the cell clears to what the terms send back.
///    Rejected, the matter is free again.
/// 2. Answer. Every offer aimed at it gets an answer: it rejects those it can't take, ranks the
///    rest (straight above, then the upper diagonals, then side offers, then the hash), accepts the
///    first and rejects the others. Acceptance is receipt: it holds the matter from now on.
/// 3. Offer. One offer of what it holds, unless that matter is promised to the offer it made this
///    phase, whose answer isn't in.
pub fn next(me: State, kept: State, seen: [State; 6], x: i32, y: i32, phase: u32, rules: &Rules) -> State {
    let at = |d: Dir| seen[d as usize];
    // Answers are made afresh each update, and an offer stands for one phase.
    let mut s = State { offer: None, terms: Material::Air, accept: None, rejects: 0, ..me };
    // The offer I published this phase is answered at the next, so its matter is promised: I
    // neither take anything in nor offer it again.
    let promised = me.offer.is_some();

    // 1. Clear.
    if let Some(d) = kept.offer {
        if at(d).accept == Some(d.opposite()) {
            s = State::holding(kept.terms);
            // The water coming back from a trade arrives still, with a hashed flow, as painted
            // water does.
            if kept.terms == Material::Water && hash(x, y, phase, SALT_FLOW) & 1 == 1 {
                s.flow = Side::Right;
            }
        } else if s.material == Material::Water && drop(x, y, d) < 0 {
            // My side step was turned down: I flip, and try the other way.
            s.flow = s.flow.flip();
        }
    }

    // 2. Answer. Air takes a fall from above, or a side step from below where it could pass the
    // water on downhill, so water never climbs. Water takes sand on terms that the water goes back:
    // a trade. Nothing else takes anything, and nothing promised does.
    let passes_on = |from: Dir| {
        let far = match from {
            Dir::DownLeft => Dir::DownRight,
            Dir::DownRight => Dir::DownLeft,
            _ => return false,
        };
        at(Dir::Down).material == Material::Air || at(far).material == Material::Air
    };
    let offered = |d: Dir| at(d).offer == Some(d.opposite());
    let takes = |d: Dir| {
        let n = at(d);
        !promised
            && match (s.material, n.terms) {
                (Material::Air, Material::Air) => drop(x, y, d) < 0 || passes_on(d),
                (Material::Water, Material::Water) => n.material == Material::Sand,
                _ => false,
            }
    };
    let candidates = Dir::ALL.into_iter().filter(|&d| offered(d) && takes(d));
    let taken = pick(candidates, |d| -drop(x, y, d), hash(x, y, phase, SALT_ACCEPT));
    let rejects = Dir::ALL.into_iter().filter(|&d| offered(d) && Some(d) != taken).fold(0, |r, d| r | 1 << d as u8);
    if let Some(d) = taken {
        let n = at(d);
        // Speed travels with the matter, plus what a fall earned: a level on the gravity chance,
        // half that for a diagonal. A trade drags it to nothing; a side step was paid for when
        // offered. Water brings its flow, turned toward the side it moved to.
        let speed = if n.terms == Material::Water {
            0
        } else if drop(x, y, d) < 0 {
            let chance = if d == Dir::Up { rules.gravity } else { rules.gravity / 2 };
            let earned = within(hash(x, y, phase, SALT_EARN), chance) as u8;
            (n.speed + earned).min(TOP_SPEED)
        } else {
            n.speed
        };
        let flow = if n.material == Material::Water { Side::of(d.opposite()).unwrap_or(n.flow) } else { Side::Left };
        s = State { material: n.material, accept: Some(d), speed, flow, ..State::AIR };
    }
    s.rejects = rejects;

    // 3. Offer.
    if promised {
        return s;
    }
    // A free fall straight down into air goes on a chance that grows with the speed, so matter
    // starts slow and builds up. Slides and trades don't wait.
    let starts = |d: Dir, speed: u8| {
        let free_fall = d == Dir::Down && at(d).material == Material::Air;
        !free_fall || within(hash(x, y, phase, SALT_START), rules.start[speed as usize])
    };
    match s.material {
        Material::Sand => {
            // The steepest downhill neighbor holding air, or still water to trade with (water
            // making no offer of its own); ties are hashed. Nowhere to go: landed, and sand stops
            // dead.
            let open = |d: Dir| {
                let n = at(d);
                drop(x, y, d) > 0
                    && (n.material == Material::Air || (n.material == Material::Water && n.offer.is_none()))
            };
            match pick(Dir::ALL.into_iter().filter(|&d| open(d)), |d| drop(x, y, d), hash(x, y, phase, SALT_OFFER)) {
                Some(d) if starts(d, s.speed) => {
                    s.offer = Some(d);
                    s.terms = at(d).material;
                }
                Some(_) => {}
                None => s.speed = 0,
            }
        }
        Material::Water => {
            // Fall: straight down first, then the diagonal on the flow side.
            let flow = s.flow;
            let falls = Dir::ALL.into_iter().filter(|&d| drop(x, y, d) > 0 && at(d).material == Material::Air);
            if let Some(d) = pick(falls, |d| 2 * drop(x, y, d) + (Side::of(d) == Some(flow)) as i32, 0) {
                if starts(d, s.speed) {
                    s.offer = Some(d);
                }
            } else {
                // Landed: water keeps at most the splash. Side step: the flow side first, else the
                // other, which becomes the flow. Pushed, by matter above it or above it on the side
                // it leaves, the step is free; otherwise it costs a level of speed, paid as the offer
                // is made, and with none left the water rests. Matter moving into me doesn't push:
                // matter offered to me, or the giver I just took from, still showing what is mine.
                s.speed = s.speed.min(rules.splash);
                // Water that can't fall takes heavier matter before it steps aside: with sand above
                // it, it stays still for the sand to trade into it. (Stepping aside, it would be
                // promised each time the sand's offer arrived, and the two would lock in step.)
                let laden = [Dir::Up, Dir::UpLeft, Dir::UpRight].into_iter().any(|d| at(d).material == Material::Sand);
                let pushes = |d: Dir| moves(at(d).material) && !offered(d) && me.accept != Some(d);
                let pushed = |side: Side| pushes(Dir::Up) || pushes(side.flip().up());
                let speed = s.speed;
                let step =
                    |side: &Side| !laden && at(side.up()).material == Material::Air && (speed > 0 || pushed(*side));
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
/// The older word is also where a cell finds the offer it is waiting on: the one it made last phase,
/// answered in what its neighbors publish now.
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

    /// What the cell published the phase before: its kept-back value, holding the offer whose
    /// answer its neighbors publish now.
    pub fn kept(&self, x: i32, y: i32) -> State {
        State::decode(self.cells[self.current(self.index(x, y)) ^ 1] & STATE_MASK)
    }

    pub fn material(&self, x: i32, y: i32) -> Material {
        self.cell(x, y).material
    }

    pub fn phase(&self, x: i32, y: i32) -> u32 {
        self.cells[self.current(self.index(x, y))] >> STATE_BITS
    }

    /// The neighbor that accepted this cell's offer: its acceptance names this cell. Then the
    /// matter the cell still shows is the receiver's, and the cell clears on its next update.
    pub fn taker(&self, x: i32, y: i32) -> Option<Dir> {
        Dir::ALL.into_iter().find(|&d| {
            let (nx, ny) = neighbor(x, y, d);
            self.in_bounds(nx, ny) && self.cell(nx, ny).accept == Some(d.opposite())
        })
    }

    /// What the observer sees: what the cell owns. A giver whose offer was accepted reads as what
    /// its terms send back: air, or the water coming back from a trade. A world drawn this way holds
    /// every unit of matter exactly once.
    pub fn looks(&self, x: i32, y: i32) -> Material {
        if self.taker(x, y).is_some() { self.kept(x, y).terms } else { self.material(x, y) }
    }

    /// How many cells' worth of `material` the world owns, a giver whose offer was accepted
    /// counting as what its terms send back.
    pub fn count(&self, material: Material) -> usize {
        (0..self.height).map(|y| (0..self.width).filter(|&x| self.looks(x, y) == material).count()).sum()
    }

    /// Whether every cell is at the same phase, as after a whole `step`.
    pub fn uniform(&self) -> bool {
        let p = self.phase(0, 0);
        (0..self.height).all(|y| (0..self.width).all(|x| self.phase(x, y) == p))
    }

    /// The contract rules, checked between the phase every cell kept back and the one it just
    /// published (so on a world whose cells share a phase, after a step):
    ///
    /// - Rule 1: an offer comes from a cell holding matter that moves, not already promised to the
    ///   offer it made the phase before.
    /// - Every offer gets exactly one answer, an acceptance or a rejection, and nothing is answered
    ///   that wasn't offered.
    /// - Rule 2: every acceptance names a giver that offered exactly that matter, to it.
    ///
    /// Conservation is `step`'s to check, since it compares counts across the step.
    pub fn audit(&self) -> Result<(), String> {
        let fail = |x: i32, y: i32, why: &str| Err(format!("({x}, {y}): {why}\n{}", self.ascii_intents()));
        for y in 0..self.height {
            for x in 0..self.width {
                let (was, now) = (self.kept(x, y), self.cell(x, y));
                if let Some(d) = now.offer {
                    if !moves(now.material) {
                        return fail(x, y, "an offer of matter it doesn't hold");
                    }
                    if was.offer.is_some() {
                        return fail(x, y, "an offer of matter already promised");
                    }
                    let (tx, ty) = neighbor(x, y, d);
                    if !self.in_bounds(tx, ty) {
                        return fail(x, y, "an offer out of the lattice");
                    }
                }
                if let Some(d) = was.offer {
                    let (tx, ty) = neighbor(x, y, d);
                    let answer = if self.in_bounds(tx, ty) { self.cell(tx, ty) } else { State::BEDROCK };
                    let named = answer.accept == Some(d.opposite());
                    if named == answer.rejected(d.opposite()) {
                        return fail(
                            x,
                            y,
                            if named { "an offer both accepted and rejected" } else { "an offer unanswered" },
                        );
                    }
                    if named && answer.material != was.material {
                        return fail(x, y, "an acceptance of other matter than was offered");
                    }
                }
                let answered = Dir::ALL.into_iter().filter(|&d| now.rejected(d)).chain(now.accept);
                for d in answered {
                    let (gx, gy) = neighbor(x, y, d);
                    if !self.in_bounds(gx, gy) || self.kept(gx, gy).offer != Some(d.opposite()) {
                        return fail(x, y, "an answer to an offer never made");
                    }
                }
            }
        }
        Ok(())
    }

    /// The brush, the one source and sink of matter: the cell now holds `material` and nothing
    /// in flight. Matter goes only into a cell holding air and no acceptance; erasing (painting air)
    /// clears any matter but bedrock. It honors open contracts: a giver whose offer was accepted is
    /// left alone, and the cell's answers stand, so an erased receiver keeps its acceptance, on air,
    /// for a phase, and its giver clears rather than take the matter back. Call it between phases.
    pub fn paint(&mut self, x: i32, y: i32, material: Material) {
        let here = self.cell(x, y);
        let allowed = match (material, here.material) {
            (Material::Air, m) => m != Material::Air && m != Material::Bedrock,
            (_, m) => m == Material::Air && here.accept.is_none(),
        };
        if !allowed || self.taker(x, y).is_some() {
            return;
        }
        // Nothing but the cell's own offer changes, and no neighbor reads that until the next phase,
        // so each cell decides its own paint whatever order the brush goes in.
        let mut s = State { accept: here.accept, rejects: here.rejects, ..State::holding(material) };
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

    /// Sets a cell's kept-back state outright, for tests that need an offer waiting on its answer.
    #[cfg(test)]
    fn put_kept(&mut self, x: i32, y: i32, s: State) {
        let i = self.current(self.index(x, y)) ^ 1;
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
        self.cells[2 * i + 1 - parity] = word(next(me, kept, seen, x, y, phase, &self.rules), ahead);
        true
    }

    /// One phase for every cell, row by row. Any order gives the same result. Debug builds audit
    /// the contract rules and conservation after every step.
    pub fn step(&mut self) {
        #[cfg(debug_assertions)]
        let before = [Material::Sand, Material::Water].map(|m| self.count(m));
        for y in 0..self.height {
            for x in 0..self.width {
                self.update(x, y);
            }
        }
        #[cfg(debug_assertions)]
        {
            if let Err(e) = self.audit() {
                panic!("contract broken after a step at phase {}: {e}", self.phase(0, 0));
            }
            let after = [Material::Sand, Material::Water].map(|m| self.count(m));
            assert_eq!(before, after, "matter not conserved by a step at phase {}", self.phase(0, 0));
        }
    }
}
