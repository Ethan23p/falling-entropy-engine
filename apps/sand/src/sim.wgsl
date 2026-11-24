// The sim on the GPU: a line-for-line mirror of crates/sim (`next`, `World::update`,
// `World::paint`). Keep the two identical; the GPU parity test checks them bit for bit.
//
// A cell keeps two words, one per phase parity: word 2i + (p & 1) holds cell i's state at phase p
// (bits 0-18) and p itself (bits 19-31, wrapping). A neighbor at phase p reads that word, which the
// cell doesn't overwrite until it reaches p + 2, so cells run in any order and in place.
// A state (`State::encode`): the material in bits 0-1 (0 air, 1 sand, 2 bedrock).
// Outside the lattice reads as bedrock.
// Directions, clockwise from up: 0 up, 1 up-right, 2 down-right, 3 down, 4 down-left, 5 up-left.

struct Params {
    width: i32,
    height: i32,
    stamps: u32,
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
const BEDROCK: u32 = 2u;

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

fn inside(n: vec2<i32>) -> bool {
    return n.x >= 0 && n.y >= 0 && n.x < params.width && n.y < params.height;
}

fn material_of(s: u32) -> u32 {
    return min(s & 3u, BEDROCK);
}

// One cell's update (`next`). No stages yet: it publishes what it held.
fn next(me: u32, kept: u32, seen: ptr<function, array<u32, 6>>, x: i32, y: i32, phase: u32) -> u32 {
    return me;
}

// The index of cell i's current word: the one tagged with the later phase.
fn current(i: i32) -> i32 {
    let even = atomicLoad(&cells[2 * i]) >> STATE_BITS;
    let odd = atomicLoad(&cells[2 * i + 1]) >> STATE_BITS;
    return select(2 * i + 1, 2 * i, ((even - odd) & PHASE_MASK) == 1u);
}

// One phase for one cell (`World::update`). Cells run in any order and in place: each reads its
// neighbors' words of its own phase's parity, and its own two words, and writes over the older.
@compute @workgroup_size(8, 8)
fn step(@builtin(global_invocation_id) id: vec3<u32>) {
    let x = i32(id.x);
    let y = i32(id.y);
    if x >= params.width || y >= params.height {
        return;
    }
    let i = y * params.width + x;
    let phase = atomicLoad(&cells[current(i)]) >> STATE_BITS;
    let parity = i32(phase & 1u);
    var seen = array<u32, 6>(BEDROCK, BEDROCK, BEDROCK, BEDROCK, BEDROCK, BEDROCK);
    for (var d = 0u; d < 6u; d++) {
        let n = neighbor(x, y, d);
        if !inside(n) {
            continue;
        }
        let nw = atomicLoad(&cells[2 * (n.y * params.width + n.x) + parity]);
        if (nw >> STATE_BITS) != phase {
            return; // a neighbor is behind
        }
        seen[d] = nw & STATE_MASK;
    }
    let me = atomicLoad(&cells[2 * i + parity]) & STATE_MASK;
    let kept = atomicLoad(&cells[2 * i + 1 - parity]) & STATE_MASK;
    let ahead = (phase + 1u) & PHASE_MASK;
    atomicStore(&cells[2 * i + 1 - parity], next(me, kept, &seen, x, y, phase) | (ahead << STATE_BITS));
}

// The brush (`World::paint`), between phases: matter goes only into cells holding air; erasing
// clears anything but bedrock. Stamps apply in order.
@compute @workgroup_size(8, 8)
fn brush(@builtin(global_invocation_id) id: vec3<u32>) {
    let x = i32(id.x);
    let y = i32(id.y);
    if x >= params.width || y >= params.height {
        return;
    }
    let centre = vec2(f32(x), f32(y) + 0.5 * f32(x & 1));
    let slot = current(y * params.width + x);
    let word = atomicLoad(&cells[slot]);
    var state = word & STATE_MASK;
    var painted = false;
    for (var s = 0u; s < params.stamps; s++) {
        let stamp = stamps[s];
        let material = u32(stamp.material);
        let off = centre - vec2(stamp.x, stamp.y);
        let here = material_of(state);
        if off.x * off.x + off.y * off.y <= stamp.radius * stamp.radius
            && select(here == AIR, here != BEDROCK && here != AIR, material == AIR) {
            state = material;
            painted = true;
        }
    }
    if painted {
        atomicStore(&cells[slot], state | (word & ~STATE_MASK));
    }
}
