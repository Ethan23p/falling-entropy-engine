// The sim on the GPU: a line-for-line mirror of crates/sim (`next`, `World::update`,
// `World::paint`). Keep the two identical; the GPU parity test checks them bit for bit.
//
// A cell's word: state (bits 0-6), kept-back value (bits 7-13), phase (bits 14-31).
// A state: material (bit 0: 0 air, 1 sand), offer (bits 1-3), accept (bits 4-6);
// a direction is stored as 1 + its index, 0 meaning none.
// Directions, clockwise from up: 0 up, 1 up-right, 2 down-right, 3 down, 4 down-left, 5 up-left.

struct Params {
    width: i32,
    height: i32,
    stamps: u32,
    _pad: u32,
}

// A brush stamp: centre (cell units, x and the half-cell-shifted y), radius, material.
struct Stamp {
    x: f32,
    y: f32,
    radius: f32,
    material: f32,
}

@group(0) @binding(0) var<uniform> params: Params;
@group(0) @binding(1) var<storage, read_write> cells: array<atomic<u32>>;
@group(0) @binding(2) var<storage, read> stamps: array<Stamp>;

const STATE_BITS: u32 = 7u;
const STATE_MASK: u32 = 127u;
const PHASE_SHIFT: u32 = 14u;
const PHASE_MASK: u32 = 0x3ffffu;
const SOLID: u32 = 0xffffffffu;
const SAND: u32 = 1u;
const SALT_OFFER: u32 = 1u;
const SALT_ACCEPT: u32 = 2u;

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

fn height(x: i32, y: i32) -> i32 {
    return -(2 * y + (x & 1));
}

fn fall(x: i32, y: i32, d: u32) -> i32 {
    let n = neighbor(x, y, d);
    return height(x, y) - height(n.x, n.y);
}

fn opposite(d: u32) -> u32 {
    return (d + 3u) % 6u;
}

fn offer_of(s: u32) -> u32 {
    return (s >> 1u) & 7u;
}

fn accept_of(s: u32) -> u32 {
    return (s >> 4u) & 7u;
}

// `pick` in crates/sim: the candidate with the highest key, equals chosen by `bits`.
// Returns 1 + the direction, or 0 for none.
struct Picker {
    best: array<u32, 6>,
    n: u32,
    key: i32,
}

fn consider(p: ptr<function, Picker>, d: u32, k: i32) {
    if k > (*p).key {
        (*p).key = k;
        (*p).n = 0u;
    }
    if k == (*p).key {
        (*p).best[(*p).n] = d;
        (*p).n += 1u;
    }
}

fn chosen(p: ptr<function, Picker>, bits: u32) -> u32 {
    if (*p).n == 0u {
        return 0u;
    }
    return (*p).best[bits % (*p).n] + 1u;
}

fn next(me: u32, seen: ptr<function, array<u32, 6>>, x: i32, y: i32, phase: u32) -> u32 {
    let offer = offer_of(me);
    let accept = accept_of(me);

    // Transfer, the giving half.
    if offer != 0u {
        let d = offer - 1u;
        let n = (*seen)[d];
        if n != SOLID && accept_of(n) == opposite(d) + 1u {
            return 0u;
        }
    }

    var material = me & 1u;
    var new_offer = offer;
    var new_accept = accept;
    // Transfer, the receiving half.
    if accept != 0u {
        new_accept = 0u;
        let d = accept - 1u;
        let n = (*seen)[d];
        if n != SOLID && offer_of(n) == opposite(d) + 1u {
            material = n & 1u;
        }
    }

    if material == 0u {
        if accept == 0u {
            var p = Picker(array<u32, 6>(0u, 0u, 0u, 0u, 0u, 0u), 0u, -2147483647 - 1);
            for (var d = 0u; d < 6u; d++) {
                let n = (*seen)[d];
                if n != SOLID && (n & 1u) == SAND && offer_of(n) == opposite(d) + 1u {
                    consider(&p, d, -fall(x, y, d));
                }
            }
            new_accept = chosen(&p, hash(x, y, phase, SALT_ACCEPT));
        }
    } else {
        new_accept = 0u;
        if new_offer != 0u {
            let d = new_offer - 1u;
            let n = (*seen)[d];
            let open = n != SOLID && (n & 1u) == 0u && (accept_of(n) == 0u || accept_of(n) == opposite(d) + 1u);
            if !open {
                new_offer = 0u;
            }
        }
        if new_offer == 0u {
            var p = Picker(array<u32, 6>(0u, 0u, 0u, 0u, 0u, 0u), 0u, -2147483647 - 1);
            for (var d = 0u; d < 6u; d++) {
                let n = (*seen)[d];
                if fall(x, y, d) > 0 && n != SOLID && (n & 1u) == 0u && accept_of(n) == 0u {
                    consider(&p, d, fall(x, y, d));
                }
            }
            new_offer = chosen(&p, hash(x, y, phase, SALT_OFFER));
        }
    }
    return material | (new_offer << 1u) | (new_accept << 4u);
}

// One phase for one cell (`World::update`). Cells run in any order and in place: a neighbor
// already a phase ahead is read through its kept-back value.
@compute @workgroup_size(8, 8)
fn step(@builtin(global_invocation_id) id: vec3<u32>) {
    let x = i32(id.x);
    let y = i32(id.y);
    if x >= params.width || y >= params.height {
        return;
    }
    let i = y * params.width + x;
    let word = atomicLoad(&cells[i]);
    let phase = word >> PHASE_SHIFT;
    var seen = array<u32, 6>(SOLID, SOLID, SOLID, SOLID, SOLID, SOLID);
    for (var d = 0u; d < 6u; d++) {
        let n = neighbor(x, y, d);
        if n.x < 0 || n.y < 0 || n.x >= params.width || n.y >= params.height {
            continue;
        }
        let nw = atomicLoad(&cells[n.y * params.width + n.x]);
        let ahead = ((nw >> PHASE_SHIFT) - phase) & PHASE_MASK;
        if ahead == 0u {
            seen[d] = nw & STATE_MASK;
        } else if ahead == 1u {
            seen[d] = (nw >> STATE_BITS) & STATE_MASK;
        } else {
            return; // a neighbor is behind
        }
    }
    let me = word & STATE_MASK;
    let after = next(me, &seen, x, y, phase);
    atomicStore(&cells[i], after | (me << STATE_BITS) | (((phase + 1u) & PHASE_MASK) << PHASE_SHIFT));
}

// The brush (`World::paint`), between phases: every cell inside a stamp holds its material
// and nothing in flight. Later stamps win.
@compute @workgroup_size(8, 8)
fn brush(@builtin(global_invocation_id) id: vec3<u32>) {
    let x = i32(id.x);
    let y = i32(id.y);
    if x >= params.width || y >= params.height {
        return;
    }
    let centre = vec2(f32(x), f32(y) + 0.5 * f32(x & 1));
    var painted = -1;
    for (var s = 0u; s < params.stamps; s++) {
        let stamp = stamps[s];
        if distance(centre, vec2(stamp.x, stamp.y)) <= stamp.radius {
            painted = i32(stamp.material);
        }
    }
    if painted < 0 {
        return;
    }
    let i = y * params.width + x;
    let phase = atomicLoad(&cells[i]) >> PHASE_SHIFT;
    let state = u32(painted);
    atomicStore(&cells[i], state | (state << STATE_BITS) | (phase << PHASE_SHIFT));
}
