// The sim on the GPU: a line-for-line mirror of crates/sim (`update::next`, `World::update`,
// `World::paint`, `physics`). Keep the two identical; the GPU parity test checks them bit for bit.
//
// A cell keeps four words: per phase parity, a state word and a velocity word. Word 4i + (p & 1)
// holds cell i's state at phase p (bits 0-18) and p itself (bits 19-31, wrapping); word
// 4i + 2 + (p & 1) holds its velocity then. A neighbor at phase p reads those words, which the cell
// doesn't overwrite until it reaches p + 2, so cells run in any order and in place.
// A state (`State::encode`): the material in bits 0-1 (0 air, 1 sand, 2 bedrock), the offer in
// bits 2-4 and the acceptance in bits 5-7 (1 + a direction, or 0 for none).
// A velocity (`Velocity::encode`): x in the low 16 bits, y in the high, both signed.
// Outside the lattice reads as bedrock.
// Directions, clockwise from up: 0 up, 1 up-right, 2 down-right, 3 down, 4 down-left, 5 up-left.

// `Physics` in crates/sim, after the lattice's size and the stamp count.
struct Params {
    width: i32,
    height: i32,
    stamps: u32,
    sand_into_air: u32,
    gravity: u32,
    mass_air: u32,
    mass_sand: u32,
    restless: u32,
    align: u32,
    into_air: u32,
    into_sand: u32,
    _pad: u32,
}

// A brush stamp: centre (cell units, x and the half-cell-shifted y), radius, material
// (0 air, 1 sand).
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
const SALT_EVALUATE: u32 = 1u;
const SALT_PROPOSE: u32 = 2u;
const ALIGN_CAP: u32 = 1048576u;
const FACTOR_CAP: u32 = 255u;

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

// A cell's published state, decoded.
struct State {
    material: u32,
    velocity: vec2<i32>,
    // 1 + a direction, or 0 for none.
    offer: u32,
    accept: u32,
}

fn decode(bits: u32, v: u32) -> State {
    let material = min(bits & 3u, BEDROCK);
    let velocity = vec2(bitcast<i32>(v << 16u) >> 16u, bitcast<i32>(v) >> 16u);
    var offer = (bits >> 2u) & 7u;
    var accept = (bits >> 5u) & 7u;
    if offer == 7u {
        offer = 0u;
    }
    if accept == 7u {
        accept = 0u;
    }
    return State(material, velocity, offer, accept);
}

fn encode(s: State) -> u32 {
    return s.material | (s.offer << 2u) | (s.accept << 5u);
}

fn encode_velocity(v: vec2<i32>) -> u32 {
    return (bitcast<u32>(v.x) & 0xffffu) | (bitcast<u32>(v.y) << 16u);
}

fn holding(m: u32) -> State {
    return State(m, vec2(0, 0), 0u, 0u);
}

// `Velocity::along`.
fn along(v: vec2<i32>, d: u32) -> i32 {
    let s = step_of(d);
    return v.x * s.x + v.y * s.y;
}

// The physics distribution (`physics` in crates/sim).

fn mass(m: u32) -> u32 {
    switch m {
        case 0u: { return params.mass_air; }
        case 1u: { return params.mass_sand; }
        default: { return 0u; }
    }
}

// `Physics::compatible`.
fn compatible(origin: u32, target: u32) -> bool {
    if origin != SAND {
        return false;
    }
    if target == AIR {
        return params.sand_into_air != 0u;
    }
    if target == SAND {
        return params.into_sand > 0u;
    }
    return false;
}

// `Physics::overwrites`.
fn overwrites(origin: u32, target: u32) -> bool {
    return origin == SAND && target == AIR && params.sand_into_air != 0u;
}

// `Physics::weight`.
fn weight(v: vec2<i32>, d: u32, target: u32) -> u32 {
    var factor = 0u;
    if target == AIR {
        factor = params.into_air;
    } else if target == SAND {
        factor = params.into_sand;
    }
    factor = min(factor, FACTOR_CAP);
    let a = u32(max(along(v, d), 0));
    let lean = min((min(a, ALIGN_CAP) * min(params.align, 255u)) >> 8u, ALIGN_CAP);
    return factor * (min(params.restless, ALIGN_CAP) + lean);
}

// `Physics::moved`: gravity's impulse, added on a move, saturating.
fn moved(m: u32, v: vec2<i32>) -> vec2<i32> {
    let impulse = i32(min(params.gravity, 4096u) * min(mass(m), 16u));
    return vec2(v.x, clamp(v.y + impulse, -32768, 32767));
}

// `physics::select`: 1 + the index chosen, or 0 when every weight is zero.
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

    // Resolve: the offer kept back has its verdict now. Accepted: granted an air.
    if kept.offer != 0u {
        let d = kept.offer - 1u;
        if (*seen)[d].accept == opposite(d) + 1u {
            live.material = AIR;
            live.velocity = vec2(0, 0);
        }
    }

    // Evaluate: select one offer whose conditions hold; it overwrites, with gravity's impulse.
    if !committed {
        var weights = array<u32, 6>(0u, 0u, 0u, 0u, 0u, 0u);
        for (var d = 0u; d < 6u; d++) {
            let origin = (*seen)[d];
            if origin.offer == opposite(d) + 1u && overwrites(origin.material, live.material) {
                weights[d] = max(weight(origin.velocity, opposite(d), live.material), 1u);
            }
        }
        let i = select_from(&weights, hash(x, y, phase, SALT_EVALUATE));
        if i != 0u {
            let origin = (*seen)[i - 1u];
            live.material = origin.material;
            live.velocity = moved(origin.material, origin.velocity);
            live.accept = i;
        }
    }

    // Evolve: no natural contracts in v01.00.

    // Propose: from the six neighbors, exclude the incompatible, select among the rest, weighted.
    if !committed {
        var weights = array<u32, 6>(0u, 0u, 0u, 0u, 0u, 0u);
        for (var d = 0u; d < 6u; d++) {
            let target = (*seen)[d].material;
            if compatible(live.material, target) {
                weights[d] = weight(live.velocity, d, target);
            }
        }
        live.offer = select_from(&weights, hash(x, y, phase, SALT_PROPOSE));
    }
    return live;
}

// The parity of cell i's current words: the state word tagged with the later phase.
fn current(i: i32) -> i32 {
    let even = atomicLoad(&cells[4 * i]) >> STATE_BITS;
    let odd = atomicLoad(&cells[4 * i + 1]) >> STATE_BITS;
    return select(1, 0, ((even - odd) & PHASE_MASK) == 1u);
}

fn read(i: i32, parity: i32) -> State {
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
        seen[d] = read(ni, parity);
    }
    let me = read(i, parity);
    let kept = read(i, 1 - parity);
    let ahead = (phase + 1u) & PHASE_MASK;
    let s = next(me, kept, &seen, x, y, phase);
    atomicStore(&cells[4 * i + 1 - parity], encode(s) | (ahead << STATE_BITS));
    atomicStore(&cells[4 * i + 3 - parity], encode_velocity(s.velocity));
}

// The brush (`World::paint`), between phases: matter goes only into cells holding air; erasing
// clears anything but bedrock; a painted particle is at rest. Cells engaged in a contract (an
// offer out, one awaiting its verdict, or a particle in transport) are left alone. Stamps apply
// in order.
@compute @workgroup_size(8, 8)
fn brush(@builtin(global_invocation_id) id: vec3<u32>) {
    let x = i32(id.x);
    let y = i32(id.y);
    if x >= params.width || y >= params.height {
        return;
    }
    let i = y * params.width + x;
    let parity = current(i);
    let now = read(i, parity);
    let kept = read(i, 1 - parity);
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
            && select(here == AIR, here != BEDROCK && here != AIR, material == AIR) {
            here = material;
            painted = true;
        }
    }
    if painted {
        atomicStore(&cells[4 * i + parity], encode(holding(here)) | (word & ~STATE_MASK));
        atomicStore(&cells[4 * i + 2 + parity], 0u);
    }
}
