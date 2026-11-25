// The sim on the GPU: a line-for-line mirror of crates/sim, in the same sections as its modules
// (lattice, particle, cell, physics, contract, selection, update, world, brush). Keep the two
// identical; the GPU parity test checks them bit for bit.
//
// A cell keeps four words: per phase parity, a state word and a velocity word. Word 4i + (p & 1)
// holds cell i's state at phase p (bits 0-18) and p itself (bits 19-31, wrapping); word
// 4i + 2 + (p & 1) holds its velocity then. A neighbor at phase p reads those words, which the cell
// doesn't overwrite until it reaches p + 2, so cells run in any order and in place.
// A state (`State::encode`): what the cell holds in bits 0-2 (0 air, 1 sand, 2 bedrock,
// 3 vacuum: nothing, 4 slate, 5 water), the offer in bits 3-5 and the acceptance in bits 6-8
// (1 + a direction, or 0 for none). A velocity (`Velocity::encode`): x in the low 16 bits, y in the high, both signed.
// Outside the lattice reads as bedrock.
// Directions, clockwise from up: 0 up, 1 up-right, 2 down-right, 3 down, 4 down-left, 5 up-left.

// `Physics` in crates/sim, after the lattice's size and the stamp count.
struct Params {
    width: i32,
    height: i32,
    stamps: u32,
    gravity: u32,
    mass_air: u32,
    mass_water: u32,
    mass_sand: u32,
    into_air: u32,
    into_water: u32,
    into_sand: u32,
    into_vacuum: u32,
    deflect_air: u32,
    deflect_water: u32,
    deflect_sand: u32,
    _pad0: u32,
    _pad1: u32,
}

// A brush stamp: centre (cell units, x and the half-cell-shifted y), radius, material
// (a material, as encoded).
struct Stamp {
    x: f32,
    y: f32,
    radius: f32,
    material: f32,
}

@group(0) @binding(0) var<uniform> params: Params;
@group(0) @binding(1) var<storage, read_write> cells: array<atomic<u32>>;
@group(0) @binding(2) var<storage, read> stamps: array<Stamp>;

const STATE_BITS: u32 = 19u;
const STATE_MASK: u32 = 0x7ffffu;
const PHASE_MASK: u32 = 0x1fffu;
const AIR: u32 = 0u;
const SAND: u32 = 1u;
const BEDROCK: u32 = 2u;
const VACUUM: u32 = 3u;
const SLATE: u32 = 4u;
const WATER: u32 = 5u;
const SALT_EVALUATE: u32 = 1u;
const SALT_PROPOSE: u32 = 2u;
const SALT_DEFLECT: u32 = 3u;
const FACTOR_CAP: u32 = 255u;
const MASS_CAP: u32 = 16u;
const ALONG_CAP: u32 = 1024u;
const DEFLECT_CAP: u32 = 1024u;
const SCALE: i32 = 256;

// Randomness (`random`).

fn mix(v0: u32) -> u32 {
    var v = v0;
    v ^= v >> 16u;
    v *= 0x7feb352du;
    v ^= v >> 15u;
    v *= 0x846ca68bu;
    return v ^ (v >> 16u);
}

fn hash(x: i32, y: i32, phase: u32, salt: u32) -> u32 {
    return mix(u32(x) ^ mix(u32(y) ^ mix(phase ^ mix(salt))));
}

// Lattice (`lattice`).

fn neighbor(x: i32, y: i32, d: u32) -> vec2<i32> {
    let odd = x & 1;
    switch d {
        case 0u: { return vec2(x, y - 1); }
        case 1u: { return vec2(x + 1, y - 1 + odd); }
        case 2u: { return vec2(x + 1, y + odd); }
        case 3u: { return vec2(x, y + 1); }
        case 4u: { return vec2(x - 1, y + odd); }
        default: { return vec2(x - 1, y - 1 + odd); }
    }
}

// `Dir::step`: the step toward a neighbor, in half cells, y downward.
fn step_of(d: u32) -> vec2<i32> {
    switch d {
        case 0u: { return vec2(0, -2); }
        case 1u: { return vec2(2, -1); }
        case 2u: { return vec2(2, 1); }
        case 3u: { return vec2(0, 2); }
        case 4u: { return vec2(-2, 1); }
        default: { return vec2(-2, -1); }
    }
}

fn inside(n: vec2<i32>) -> bool {
    return n.x >= 0 && n.y >= 0 && n.x < params.width && n.y < params.height;
}

fn opposite(d: u32) -> u32 {
    return (d + 3u) % 6u;
}

// Cell (`cell`): a cell's published state, decoded: its live particle and its live contracts.
struct State {
    material: u32,
    velocity: vec2<i32>,
    // 1 + a direction, or 0 for none.
    offer: u32,
    accept: u32,
}

// `Material::decode`: the codes past water read as vacuum.
fn decode_material(bits: u32) -> u32 {
    return select(VACUUM, bits, bits <= WATER);
}

fn decode(bits: u32, v: u32) -> State {
    let material = decode_material(bits & 7u);
    let velocity = vec2(bitcast<i32>(v << 16u) >> 16u, bitcast<i32>(v) >> 16u);
    var offer = (bits >> 3u) & 7u;
    var accept = (bits >> 6u) & 7u;
    if offer == 7u {
        offer = 0u;
    }
    if accept == 7u {
        accept = 0u;
    }
    return State(material, velocity, offer, accept);
}

fn encode(s: State) -> u32 {
    return s.material | (s.offer << 3u) | (s.accept << 6u);
}

fn encode_velocity(v: vec2<i32>) -> u32 {
    return (bitcast<u32>(v.x) & 0xffffu) | (bitcast<u32>(v.y) << 16u);
}

// `State::holding`: a cell holding a fresh particle (or nothing); one that moves intends to move down.
fn holding(m: u32) -> State {
    var v = vec2(0, 0);
    if moves(m) {
        v = vec2(0, SCALE);
    }
    return State(m, v, 0u, 0u);
}

// Particle (`particle`): `Material::is_particle`, `is_static`, `moves`.

fn is_particle(m: u32) -> bool {
    return m != VACUUM;
}

fn is_static(m: u32) -> bool {
    return m == SLATE || m == BEDROCK;
}

fn moves(m: u32) -> bool {
    return is_particle(m) && !is_static(m);
}

// `Velocity::steady`: the direction of v at the steady scale; down when it has none.
fn steady(v: vec2<i32>) -> vec2<i32> {
    let m = max(abs(v.x), abs(v.y));
    if m == 0 {
        return vec2(0, SCALE);
    }
    return vec2(v.x * SCALE / m, v.y * SCALE / m);
}

// `Velocity::along`.
fn along(v: vec2<i32>, d: u32) -> i32 {
    let s = step_of(d);
    return v.x * s.x + v.y * s.y;
}

// The physics distribution (`physics`): constants and pure functions only.

fn mass(m: u32) -> u32 {
    switch m {
        case 0u: { return params.mass_air; }
        case 1u: { return params.mass_sand; }
        case 5u: { return params.mass_water; }
        default: { return 0u; }
    }
}

// `Physics::factor`: the weight multiplier for offering into a cell holding dest.
fn factor(dest: u32) -> u32 {
    var f = 0u;
    if dest == AIR {
        f = params.into_air;
    } else if dest == WATER {
        f = params.into_water;
    } else if dest == SAND {
        f = params.into_sand;
    } else if dest == VACUUM {
        f = params.into_vacuum;
    }
    return min(f, FACTOR_CAP);
}

// `Physics::compatible`, propose's exclusion: particles that move offer into a neighbor holding
// nothing, or another particle that moves whose weight multiplier isn't zero; never into a static one.
fn compatible(origin: u32, dest: u32) -> bool {
    return moves(origin) && (dest == VACUUM || (moves(dest) && factor(dest) > 0u));
}

// `Physics::entertains`, evaluate's exclusion, the capacity limit: a cell holding a particle rejects
// every offer of one; holding nothing, it can take one.
fn entertains(offered: u32, dest: u32) -> bool {
    return moves(offered) && !is_particle(dest);
}

// `Physics::weight`. Provisional: zero at or behind 90 degrees from the velocity, for every material.
fn weight(m: u32, v: vec2<i32>, d: u32, dest: u32) -> u32 {
    let a = min(u32(max(along(v, d), 0)), ALONG_CAP);
    return factor(dest) * min(mass(m), MASS_CAP) * ((a * a) >> 8u);
}

// `Physics::moved`, gravity: its impulse, added on a move, then back to the steady scale.
fn moved(m: u32, v: vec2<i32>) -> vec2<i32> {
    let impulse = i32(min(params.gravity, 4096u) * min(mass(m), MASS_CAP));
    return steady(vec2(v.x, v.y + impulse));
}

// Contract (`contract`): an offer's conditions (its verdict) and its effects on each end.

// `contract::verdict`: an offer toward d is accepted if the target's verdict names this cell.
fn accepted(d: u32, dest: State) -> bool {
    return dest.accept == opposite(d) + 1u;
}

// `contract::accept`: the effect on the target. The particle from the origin toward d
// (a direction) arrives with gravity's impulse, and the verdict names that origin.
fn contract_accept(dest: ptr<function, State>, origin: State, d: u32) {
    (*dest).material = origin.material;
    (*dest).velocity = moved(origin.material, origin.velocity);
    (*dest).accept = d + 1u;
}

// `contract::resolve`: the effect on the origin. Accepted: it holds nothing. Rejected: it keeps its
// particle, deflected (provisional).
fn contract_resolve(origin: ptr<function, State>, d: u32, is_accepted: bool, bits: u32) {
    if is_accepted {
        *origin = holding(VACUUM);
    } else {
        (*origin).velocity = deflected((*origin).material, (*origin).velocity, d, bits);
    }
}

// `contract::deflected`, provisional: the rejection effect. The velocity turns away from d,
// perpendicular to it, by the material's deflection, to the side it already leans (or the side bits
// picks when dead on), then back to the steady scale.
fn deflected(m: u32, v: vec2<i32>, d: u32, bits: u32) -> vec2<i32> {
    var k = 0u;
    if m == AIR {
        k = params.deflect_air;
    } else if m == WATER {
        k = params.deflect_water;
    } else if m == SAND {
        k = params.deflect_sand;
    }
    let ki = i32(min(k, DEFLECT_CAP));
    if ki == 0 {
        return v;
    }
    let s = step_of(d);
    let lean = -v.x * s.y + v.y * s.x;
    var side = -1;
    if lean > 0 || (lean == 0 && (bits & 1u) == 1u) {
        side = 1;
    }
    return steady(vec2(v.x - side * ki * s.y / 2, v.y + side * ki * s.x / 2));
}

// Selection (`selection::select`): 1 + the index chosen, or 0 when every weight is zero.
fn select_from(weights: ptr<function, array<u32, 6>>, bits: u32) -> u32 {
    var total = 0u;
    for (var i = 0u; i < 6u; i++) {
        total += (*weights)[i];
    }
    if total == 0u {
        return 0u;
    }
    var r = bits % total;
    for (var i = 0u; i < 6u; i++) {
        let w = (*weights)[i];
        if r < w {
            return i + 1u;
        }
        r -= w;
    }
    return 0u;
}

// One update: read, resolve, evaluate, evolve, publish, propose (`update::next`).
fn next(me: State, kept: State, seen: ptr<function, array<State, 6>>, x: i32, y: i32, phase: u32) -> State {
    var live = me;
    live.offer = 0u;
    live.accept = 0u;
    let committed = me.offer != 0u;

    // Resolve: the offer kept back has its verdict now; apply the contract's effect on its origin.
    if kept.offer != 0u {
        let d = kept.offer - 1u;
        contract_resolve(&live, d, accepted(d, (*seen)[d]), hash(x, y, phase, SALT_DEFLECT));
    }

    // Evaluate: the offers that target me, excluded unless the physics distribution entertains them,
    // the rest weighted; the one selected is accepted. Holding one particle at most, I then exclude
    // the rest, which are rejected by not being named.
    if !committed {
        var weights = array<u32, 6>(0u, 0u, 0u, 0u, 0u, 0u);
        for (var d = 0u; d < 6u; d++) {
            let origin = (*seen)[d];
            if origin.offer == opposite(d) + 1u && entertains(origin.material, live.material) {
                weights[d] = max(weight(origin.material, origin.velocity, opposite(d), live.material), 1u);
            }
        }
        let i = select_from(&weights, hash(x, y, phase, SALT_EVALUATE));
        if i != 0u {
            contract_accept(&live, (*seen)[i - 1u], i - 1u);
        }
    }

    // Evolve: no natural contracts yet.

    // Publish, then propose, in one write (see `update::next`).
    // Propose: from the six neighbors, exclude the incompatible, select among the rest, weighted.
    if !committed {
        var weights = array<u32, 6>(0u, 0u, 0u, 0u, 0u, 0u);
        for (var d = 0u; d < 6u; d++) {
            let dest = (*seen)[d].material;
            if compatible(live.material, dest) {
                weights[d] = weight(live.material, live.velocity, d, dest);
            }
        }
        live.offer = select_from(&weights, hash(x, y, phase, SALT_PROPOSE));
    }
    return live;
}

// World (`world`): the cells as stored, and the synchronizer.

// The parity of cell i's current words: the state word tagged with the later phase.
fn current(i: i32) -> i32 {
    let even = atomicLoad(&cells[4 * i]) >> STATE_BITS;
    let odd = atomicLoad(&cells[4 * i + 1]) >> STATE_BITS;
    return select(1, 0, ((even - odd) & PHASE_MASK) == 1u);
}

fn load_state(i: i32, parity: i32) -> State {
    return decode(atomicLoad(&cells[4 * i + parity]) & STATE_MASK, atomicLoad(&cells[4 * i + 2 + parity]));
}

// One phase for one cell (`World::update`). Cells run in any order and in place: each reads its
// neighbors' words of its own phase's parity, and its own, and writes over its older ones.
@compute @workgroup_size(8, 8)
fn step(@builtin(global_invocation_id) id: vec3<u32>) {
    let x = i32(id.x);
    let y = i32(id.y);
    if x >= params.width || y >= params.height {
        return;
    }
    let i = y * params.width + x;
    let phase = atomicLoad(&cells[4 * i + current(i)]) >> STATE_BITS;
    let parity = i32(phase & 1u);
    var seen = array<State, 6>(
        holding(BEDROCK), holding(BEDROCK), holding(BEDROCK),
        holding(BEDROCK), holding(BEDROCK), holding(BEDROCK)
    );
    for (var d = 0u; d < 6u; d++) {
        let n = neighbor(x, y, d);
        if !inside(n) {
            continue;
        }
        let ni = n.y * params.width + n.x;
        if (atomicLoad(&cells[4 * ni + parity]) >> STATE_BITS) != phase {
            return; // a neighbor is behind
        }
        seen[d] = load_state(ni, parity);
    }
    let me = load_state(i, parity);
    let kept = load_state(i, 1 - parity);
    let ahead = (phase + 1u) & PHASE_MASK;
    let s = next(me, kept, &seen, x, y, phase);
    atomicStore(&cells[4 * i + 1 - parity], encode(s) | (ahead << STATE_BITS));
    atomicStore(&cells[4 * i + 3 - parity], encode_velocity(s.velocity));
}

// The brush (`brush`, `World::paint`), between phases: a particle is spawned only into a cell holding
// nothing; erasing
// (vacuum) empties anything but bedrock; a painted particle intends to move down. Cells engaged in
// a contract (an offer out, one awaiting its verdict, or a particle in transport) are left alone.
// Stamps apply in order.
@compute @workgroup_size(8, 8)
fn brush(@builtin(global_invocation_id) id: vec3<u32>) {
    let x = i32(id.x);
    let y = i32(id.y);
    if x >= params.width || y >= params.height {
        return;
    }
    let i = y * params.width + x;
    let parity = current(i);
    let now = load_state(i, parity);
    let kept = load_state(i, 1 - parity);
    if now.offer != 0u || now.accept != 0u || kept.offer != 0u {
        return;
    }
    let centre = vec2(f32(x), f32(y) + 0.5 * f32(x & 1));
    let word = atomicLoad(&cells[4 * i + parity]);
    var here = now.material;
    var painted = false;
    for (var s = 0u; s < params.stamps; s++) {
        let stamp = stamps[s];
        let material = u32(stamp.material);
        let off = centre - vec2(stamp.x, stamp.y);
        if off.x * off.x + off.y * off.y <= stamp.radius * stamp.radius
            && here != material && here != BEDROCK
            && (material == VACUUM || here == VACUUM) {
            here = material;
            painted = true;
        }
    }
    if painted {
        let s = holding(here);
        atomicStore(&cells[4 * i + parity], encode(s) | (word & ~STATE_MASK));
        atomicStore(&cells[4 * i + 2 + parity], encode_velocity(s.velocity));
    }
}
