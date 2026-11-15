//! The model's CPU reference: v01's engine, built beside v00's rules (`crates/sim`) until it earns
//! its place. Names follow the Primitives section of the Falling Sand Engine tab.
//!
//! Cells on a lattice hold matter. Matter has a material (density, for now) and energy, which shows
//! as velocity. Cells move matter only by directed, conditional contracts: an origin cell proposes
//! (offers its matter to a neighbor), the target answers on its next update with a verdict, accept
//! or reject, and the origin resolves on the update after that. Acceptance is receipt: the target
//! writes itself holding the matter, and the origin, reading the acceptance, becomes air.
//!
//! Each update runs Read, Resolve, Evaluate, Propose, Publish ([`next`]). A cell reads only its
//! neighbors' published state at its own phase, under local phase synchronization ([`World::update`]),
//! so a lattice of independently timed cells behaves exactly like a synchronous one.
//!
//! The rules are stupidly simple on purpose. Matter offers itself the way its velocity points, to
//! whatever neighbor is there; the target accepts if, after resolving its own offer, it holds air.
//! There are no swaps or two-way terms. Velocity is carried with the matter but never accumulates:
//! gravity sets its size, and density against the neighborhood turns it. The lattice has no down;
//! the only downward bias is that comparison, against the contents directly below and above.

pub mod debug;
pub mod scenarios;
#[cfg(test)]
mod tests;

// Lattice

/// The lattice's shape. The game's is hexagonal; the square one proves a contract on the simplest
/// loop before the hex geometry joins in.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Shape {
    Hex,
    Square,
}

/// A neighbor's place around a cell, clockwise from up: 0 to 5 on the hex lattice, 0 to 3 on the
/// square one.
pub type Slot = usize;

/// The most neighbors a cell has.
pub const MAX_SLOTS: usize = 6;

/// A direction or a velocity, `(x, y)` with `y` down, in eighths of a cell.
pub type Vector = (i32, i32);

impl Shape {
    pub fn slots(self) -> usize {
        match self {
            Shape::Hex => 6,
            Shape::Square => 4,
        }
    }

    /// The direction toward the neighbor in slot `s`, about 8 long.
    pub fn dir(self, s: Slot) -> Vector {
        match self {
            Shape::Hex => [(0, -8), (7, -4), (7, 4), (0, 8), (-7, 4), (-7, -4)][s],
            Shape::Square => [(0, -8), (8, 0), (0, 8), (-8, 0)][s],
        }
    }

    /// The slot the neighbor in slot `s` sees this cell in.
    pub fn back(self, s: Slot) -> Slot {
        (s + self.slots() / 2) % self.slots()
    }

    /// The position of the neighbor of `(x, y)` in slot `s`. On the hex lattice, odd columns sit half
    /// a cell lower than even ones, as in v00.
    pub fn neighbor(self, x: i32, y: i32, s: Slot) -> (i32, i32) {
        match self {
            Shape::Hex => {
                let odd = x & 1;
                [(x, y - 1), (x + 1, y - 1 + odd), (x + 1, y + odd), (x, y + 1), (x - 1, y + odd), (x - 1, y - 1 + odd)]
                    [s]
            }
            Shape::Square => [(x, y - 1), (x + 1, y), (x, y + 1), (x - 1, y)][s],
        }
    }
}

pub fn dot(a: Vector, b: Vector) -> i32 {
    a.0 * b.0 + a.1 * b.1
}

fn isqrt(n: i32) -> i32 {
    if n < 2 {
        return n;
    }
    let (mut x, mut y) = (n, (n + 1) / 2);
    while y < x {
        x = y;
        y = (x + n / x) / 2;
    }
    x
}

/// A velocity's magnitude.
pub fn speed(v: Vector) -> i32 {
    isqrt(dot(v, v))
}

// Randomness

/// One round of the lowbias32 integer hash (Chris Wellons), as in v00.
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

const SALT_ACCEPT: u32 = 0x10;
const SALT_OFFER: u32 = 0x20;

/// The best of `candidates` by priority, then randomly within a priority: a key per slot from the
/// hash, so there is no order-of-arrival advantage.
fn best(candidates: impl Iterator<Item = (Slot, i32)>, at: (i32, i32, u32), salt: u32) -> Option<Slot> {
    let (x, y, phase) = at;
    candidates.max_by_key(|&(s, priority)| (priority, hash(x, y, phase, salt + s as u32))).map(|(s, _)| s)
}

// Matter

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Material {
    #[default]
    Air,
    Water,
    Sand,
    Slate,
    Bedrock,
}

impl Material {
    /// Air is zero; water very light, sand mid, slate high. Bedrock is so dense nothing moves it: it
    /// never updates, so its density is never read.
    pub fn density(self) -> i32 {
        match self {
            Material::Air => 0,
            Material::Water => 1,
            Material::Sand => 2,
            Material::Slate => 3,
            Material::Bedrock => 255,
        }
    }
}

/// Substance in a cell: one per cell.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct Matter {
    pub material: Material,
    /// Energy's manifestation as motion. Matter intends to move along it.
    pub velocity: Vector,
    /// Which unit of matter this is, for measuring only: conservation is checked unit by unit, and
    /// a ring's circulation follows units around. The rules never read it.
    pub id: u16,
}

impl Matter {
    pub const AIR: Matter = Matter { material: Material::Air, velocity: (0, 0), id: 0 };
    pub const BEDROCK: Matter = Matter { material: Material::Bedrock, ..Matter::AIR };

    pub fn is_air(self) -> bool {
        self.material == Material::Air
    }
}

// State

/// A target's answer to a proposal.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Verdict {
    #[default]
    None,
    Accept,
    Reject,
}

/// What a cell publishes at the end of its update: what its neighbors read.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct State {
    pub matter: Matter,
    /// The proposals made at this phase, by slot: an offer of this cell's matter to that neighbor. A
    /// cell holds one matter, so it offers at most once: its offers never add up to more than it has.
    pub offers: [bool; MAX_SLOTS],
    /// This phase's verdicts on the proposals the neighbors published last phase, by slot.
    pub verdicts: [Verdict; MAX_SLOTS],
}

impl State {
    pub const AIR: State =
        State { matter: Matter::AIR, offers: [false; MAX_SLOTS], verdicts: [Verdict::None; MAX_SLOTS] };
    pub const BEDROCK: State = State { matter: Matter::BEDROCK, ..State::AIR };

    pub fn holding(matter: Matter) -> State {
        State { matter, ..State::AIR }
    }

    pub fn offering(&self) -> bool {
        self.offers.contains(&true)
    }

    pub fn offer(&self) -> Option<Slot> {
        self.offers.iter().position(|&o| o)
    }

    pub fn accepted(&self) -> Option<Slot> {
        self.verdicts.iter().position(|&v| v == Verdict::Accept)
    }
}

// Rules

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Rules {
    /// Gravity, in eighths of a cell per unit of density: the size of matter's velocity is its density
    /// times this. It sets no direction; the lattice has no down, only the neighborhood does.
    pub gravity: i32,
}

impl Default for Rules {
    fn default() -> Rules {
        Rules { gravity: 8 }
    }
}

const SALT_SIDE: u32 = 0x30;

/// Velocity, from the material reactions. Density against gravity sets its size: density times
/// gravity. Density against the neighborhood turns it: denser than the contents directly below, a
/// nudge perpendicular to that, sideways; lighter than the contents directly above, a strong nudge
/// the same way; else nothing. Bedrock exerts nothing. The nudge goes the way the velocity already
/// leans (the smaller turn), or either way at random. Velocity is carried, so matter with none and no
/// nudge stays at rest.
fn react(shape: Shape, rules: &Rules, at: (i32, i32, u32), matter: Matter, seen: &[State; MAX_SLOTS]) -> Vector {
    let (g, d) = (rules.gravity, matter.material.density());
    let vertical = |down: bool| (0..shape.slots()).find(|&s| shape.dir(s).0 == 0 && (shape.dir(s).1 > 0) == down);
    let contents =
        |s: Option<Slot>| s.map(|s| seen[s].matter.material).filter(|&m| m != Material::Bedrock).map(Material::density);
    let (below, above) = (contents(vertical(true)), contents(vertical(false)));
    let nudge = match (below, above) {
        (Some(b), _) if d > b => g * (d - b),
        (_, Some(a)) if d < a => 2 * g * (a - d),
        _ => 0,
    };
    let v = matter.velocity;
    let side = match v.0.signum() {
        0 => [-1, 1][(hash(at.0, at.1, at.2, SALT_SIDE) & 1) as usize],
        lean => lean,
    };
    let raw = (v.0 + side * nudge, v.1);
    if raw == (0, 0) {
        return (0, 0);
    }
    let (size, len) = (g * d, speed(raw));
    (raw.0 * size / len, raw.1 * size / len)
}

/// One cell's update, from phase `p` to `p + 1`: its next published state, from what it published at
/// `p` (`me`) and at `p - 1` (`kept`), and what its neighbors published at `p` (`seen`, bedrock beyond
/// the edge). `at` is the cell's position and `p`.
pub fn next(
    shape: Shape,
    rules: &Rules,
    at: (i32, i32, u32),
    me: State,
    kept: State,
    seen: &[State; MAX_SLOTS],
) -> State {
    let slots = 0..shape.slots();
    let mut matter = me.matter;

    // Resolve: the proposal this cell published last phase, on the verdict its target publishes now.
    // An acceptance is receipt, so the matter is the target's already, and this cell is air.
    if kept.offer().is_some_and(|s| seen[s].verdicts[shape.back(s)] == Verdict::Accept) {
        matter = Matter::AIR;
    }

    // Evaluate: every proposal to this cell, all together, by priority (how squarely the giver's
    // velocity points here), then randomly. A cell holding air has room for one, and writes itself
    // holding it; a cell holding matter has none.
    let proposals = || slots.clone().filter(|&s| seen[s].offers[shape.back(s)]);
    let chosen = if matter.is_air() {
        let priority = |s: Slot| dot(seen[s].matter.velocity, shape.dir(shape.back(s)));
        best(proposals().map(|s| (s, priority(s))), at, SALT_ACCEPT)
    } else {
        None
    };
    let mut verdicts = [Verdict::None; MAX_SLOTS];
    for s in proposals() {
        verdicts[s] = if Some(s) == chosen { Verdict::Accept } else { Verdict::Reject };
    }
    // The matter received keeps its velocity's size and points the way it just moved.
    if let Some(s) = chosen {
        matter = seen[s].matter;
        let (moved, size) = (shape.dir(shape.back(s)), speed(matter.velocity));
        let len = speed(moved);
        matter.velocity = (moved.0 * size / len, moved.1 * size / len);
    }

    // Propose: gravity sets the velocity's size and the neighborhood its direction, and matter offers itself to a neighbor, bedrock
    // aside, preferring the one its velocity points at most squarely (the smallest turn), randomly
    // within a priority, unless its last offer is still unanswered.
    let mut offers = [false; MAX_SLOTS];
    if !matter.is_air() {
        matter.velocity = react(shape, rules, at, matter, seen);
        let v = matter.velocity;
        if !me.offering() {
            let along = |s: Slot| dot(v, shape.dir(s));
            let candidates = slots.clone().filter(|&s| along(s) >= 0 && seen[s].matter.material != Material::Bedrock);
            if let Some(s) = best(candidates.map(|s| (s, along(s))), at, SALT_OFFER) {
                offers[s] = true;
            }
        }
    }

    // Publish.
    State { matter, offers, verdicts }
}

// World

/// The lattice of cells, each with its phase and its last two published states.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct World {
    shape: Shape,
    width: i32,
    height: i32,
    /// Each cell's last two published states, by phase parity: its current one and the one before.
    states: Vec<[State; 2]>,
    phases: Vec<u32>,
    next_id: u16,
    pub rules: Rules,
}

impl World {
    pub fn new(shape: Shape, width: i32, height: i32) -> World {
        let n = (width * height) as usize;
        World {
            shape,
            width,
            height,
            states: vec![[State::AIR; 2]; n],
            phases: vec![0; n],
            next_id: 1,
            rules: Rules::default(),
        }
    }

    pub fn shape(&self) -> Shape {
        self.shape
    }

    pub fn width(&self) -> i32 {
        self.width
    }

    pub fn height(&self) -> i32 {
        self.height
    }

    pub fn len(&self) -> usize {
        self.states.len()
    }

    pub fn is_empty(&self) -> bool {
        self.states.is_empty()
    }

    pub fn index(&self, x: i32, y: i32) -> usize {
        (y * self.width + x) as usize
    }

    pub fn xy(&self, i: usize) -> (i32, i32) {
        (i as i32 % self.width, i as i32 / self.width)
    }

    pub fn in_bounds(&self, x: i32, y: i32) -> bool {
        (0..self.width).contains(&x) && (0..self.height).contains(&y)
    }

    /// The neighbor of cell `i` in slot `s`, if it is on the lattice.
    pub fn neighbor(&self, i: usize, s: Slot) -> Option<usize> {
        let (x, y) = self.xy(i);
        let (nx, ny) = self.shape.neighbor(x, y, s);
        self.in_bounds(nx, ny).then(|| self.index(nx, ny))
    }

    pub fn is_bedrock(&self, i: usize) -> bool {
        self.states[i][0].matter.material == Material::Bedrock
    }

    /// The cells that update: all but bedrock.
    pub fn cells(&self) -> impl Iterator<Item = usize> + '_ {
        (0..self.len()).filter(|&i| !self.is_bedrock(i))
    }

    pub fn phase(&self, i: usize) -> u32 {
        self.phases[i]
    }

    /// What the cell published at its current phase.
    pub fn cell(&self, i: usize) -> State {
        self.states[i][(self.phases[i] & 1) as usize]
    }

    /// What the cell published the phase before: it holds the proposals whose verdicts the
    /// neighbors publish now.
    pub fn kept(&self, i: usize) -> State {
        self.states[i][(!self.phases[i] & 1) as usize]
    }

    /// What the cell's neighbor in slot `s` published, at that neighbor's current phase; bedrock beyond
    /// the edge.
    pub fn seen(&self, i: usize, s: Slot) -> State {
        self.neighbor(i, s).map_or(State::BEDROCK, |n| self.cell(n))
    }

    /// Places matter in a cell, before the world runs or between whole phases; each unit gets its
    /// own id. Bedrock fills a cell that never updates.
    pub fn place(&mut self, x: i32, y: i32, material: Material, velocity: Vector) {
        let id = match material {
            Material::Air | Material::Bedrock => 0,
            _ => {
                self.next_id += 1;
                self.next_id - 1
            }
        };
        let i = self.index(x, y);
        self.states[i] = [State::holding(Matter { material, velocity, id }); 2];
    }

    /// Sets a cell's published state and the one before it, before the world runs: for starting a
    /// scenario mid-contract.
    pub fn seed(&mut self, i: usize, now: State, kept: State) {
        self.states[i] = [now, kept];
    }

    /// One cell's update. It advances a phase once every neighbor has matched or passed its phase,
    /// reading each neighbor's state of its own phase's parity, and overwrites the older of its own
    /// two states, which no neighbor reads any more. Returns whether it advanced.
    pub fn update(&mut self, i: usize) -> bool {
        if self.is_bedrock(i) {
            return false;
        }
        let p = self.phases[i];
        let parity = (p & 1) as usize;
        let mut seen = [State::BEDROCK; MAX_SLOTS];
        for (s, seen) in seen.iter_mut().enumerate().take(self.shape.slots()) {
            let Some(n) = self.neighbor(i, s) else { continue };
            if self.is_bedrock(n) {
                continue;
            }
            if self.phases[n] < p {
                return false; // a neighbor is behind
            }
            *seen = self.states[n][parity];
        }
        let (me, kept) = (self.states[i][parity], self.states[i][1 - parity]);
        let (x, y) = self.xy(i);
        self.states[i][1 - parity] = next(self.shape, &self.rules, (x, y, p), me, kept, &seen);
        self.phases[i] = p + 1;
        true
    }

    /// One phase for every cell, in `order`. From a world whose cells share a phase, one pass in any
    /// order advances them all, to the same result.
    pub fn step_in(&mut self, order: impl IntoIterator<Item = usize>) {
        for i in order {
            self.update(i);
        }
        debug_assert!(self.uniform(), "a whole pass leaves every cell at the next phase");
    }

    /// One phase for every cell, row by row.
    pub fn step(&mut self) {
        self.step_in(0..self.len());
    }

    /// Whether every cell is at the same phase, as after a whole step.
    pub fn uniform(&self) -> bool {
        let mut phases = self.cells().map(|i| self.phases[i]);
        let first = phases.next();
        phases.all(|p| Some(p) == first)
    }

    /// The common phase of a uniform world.
    pub fn now(&self) -> u32 {
        self.cells().next().map_or(0, |i| self.phases[i])
    }

    /// Whether a proposal the cell published last phase was accepted in what its targets publish now.
    /// Then its matter is the target's, though the cell still shows it until it resolves.
    pub fn given(&self, i: usize) -> bool {
        let kept = self.kept(i);
        (0..self.shape.slots())
            .any(|s| kept.offers[s] && self.seen(i, s).verdicts[self.shape.back(s)] == Verdict::Accept)
    }

    /// The matter the cell owns, in published state: a giver whose proposal was accepted owns air.
    pub fn owned(&self, i: usize) -> Matter {
        // A giver whose offer was accepted owns nothing, unless it accepted matter in the same update.
        let received = self.cell(i).accepted().is_some();
        if !self.is_bedrock(i) && self.given(i) && !received { Matter::AIR } else { self.cell(i).matter }
    }

    /// The ids of every unit of matter the world owns, sorted, as published. Conservation is this
    /// staying the same.
    pub fn inventory(&self) -> Vec<u16> {
        let mut ids: Vec<u16> = self.cells().map(|i| self.owned(i)).filter(|m| !m.is_air()).map(|m| m.id).collect();
        ids.sort_unstable();
        ids
    }

    /// The same, counting what every cell shows rather than owns: during the phase after an acceptance
    /// the matter shows in both the giver and the receiver.
    pub fn shown(&self) -> Vec<u16> {
        let mut ids: Vec<u16> =
            self.cells().map(|i| self.cell(i).matter).filter(|m| !m.is_air()).map(|m| m.id).collect();
        ids.sort_unstable();
        ids
    }

    /// Matter in transit: shown by a giver whose proposal was accepted, and by its receiver.
    pub fn in_transit(&self) -> usize {
        self.cells().filter(|&i| self.given(i)).count()
    }

    /// The contract rules, checked on a uniform world, between the state every cell kept and the one
    /// it just published:
    ///
    /// - An offer comes from a cell holding matter, whose last offer isn't still unanswered, to a
    ///   neighbor on the lattice that isn't bedrock.
    /// - Every proposal gets exactly one verdict, and every verdict answers a proposal.
    /// - A cell accepts at most one proposal, and then holds exactly the matter offered.
    pub fn audit(&self) -> Result<(), String> {
        if !self.uniform() {
            return Err("audit wants a uniform world".into());
        }
        let shape = self.shape;
        for i in self.cells() {
            let fail =
                |why: &str| Err(format!("{:?} at phase {}: {why}\n{}", self.xy(i), self.now(), self.ascii_intents()));
            let (now, was) = (self.cell(i), self.kept(i));
            if now.offers.iter().filter(|&&o| o).count() > 1 {
                return fail("more offers than the one matter it holds");
            }
            if now.offering() && (now.matter.is_air() || was.offering()) {
                return fail("an offer of matter it doesn't hold, or whose last offer is unanswered");
            }
            for s in 0..shape.slots() {
                let target = self.neighbor(i, s).filter(|&t| !self.is_bedrock(t));
                if now.offers[s] && target.is_none() {
                    return fail("an offer to bedrock");
                }
                if was.offers[s] && self.seen(i, s).verdicts[shape.back(s)] == Verdict::None {
                    return fail("a proposal without a verdict");
                }
                if now.verdicts[s] != Verdict::None {
                    let Some(g) = target.filter(|&g| self.kept(g).offers[shape.back(s)]) else {
                        return fail("a verdict on a proposal never made");
                    };
                    let offered = self.kept(g).matter;
                    if now.verdicts[s] == Verdict::Accept
                        && (now.matter.id, now.matter.material) != (offered.id, offered.material)
                    {
                        return fail("an acceptance holding other matter than was offered");
                    }
                }
            }
            if now.verdicts.iter().filter(|&&v| v == Verdict::Accept).count() > 1 {
                return fail("more than one acceptance");
            }
        }
        Ok(())
    }

    /// Conservation, measured on published state: the world owns exactly the units in `expected`.
    pub fn conserves(&self, expected: &[u16]) -> Result<(), String> {
        let got = self.inventory();
        if got == expected {
            return Ok(());
        }
        let extra: Vec<u16> = got.iter().filter(|id| !expected.contains(id)).copied().collect();
        let doubled: Vec<u16> = got.windows(2).filter(|w| w[0] == w[1]).map(|w| w[0]).collect();
        let missing: Vec<u16> = expected.iter().filter(|id| !got.contains(id)).copied().collect();
        Err(format!(
            "matter not conserved at phase {}: doubled {doubled:?}, missing {missing:?}, new {extra:?}\n{}",
            self.now(),
            self.ascii_intents()
        ))
    }
}
