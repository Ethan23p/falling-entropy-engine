// The sim on the GPU: a line-for-line mirror of crates/sim (`next`, `World::update`,
// `World::paint`). Keep the two identical; the GPU parity test checks them bit for bit.
//
// A cell keeps two words, one per phase parity: word 2i + (p & 1) holds cell i's state at phase p
// (bits 0-18) and p itself (bits 19-31, wrapping). A neighbor at phase p reads that word, which the
// cell doesn't overwrite until it reaches p + 2, so cells run in any order and in place. The older
// word also holds the offer the cell is waiting on: the one it made last phase.
// A state (`State::encode`): contents (bits 0-2: 0 air, 1 sand, 2 water, 3 slate, 4 bedrock); the
// offer's direction (bits 3-5, 1 + a direction, or 0 for none) and terms (bit 6, set when water comes
// back); the acceptance's direction (bits 7-9, as the offer's); the rejections (bits 10-15, bit
// 10 + d for an offer from d); speed (bits 16-17); flow (bit 18, set for right).
// Outside the lattice reads as bedrock.
// Directions, clockwise from up: 0 up, 1 up-right, 2 down-right, 3 down, 4 down-left, 5 up-left.

// `Rules` in crates/sim: chances out of 65,536, worked out on the CPU.
struct Params {
    width: i32,
    height: i32,
    stamps: u32,
    splash: u32,
    start: vec4<u32>,
    gravity: u32,
    friction: u32,
    _pad0: u32,
    _pad1: u32,
}

// A brush stamp: centre (cell units, x and the half-cell-shifted y), radius, material
// (0 air, 1 sand, 2 water, 3 slate).
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
const WATER: u32 = 2u;
const BEDROCK: u32 = 4u;
const SALT_OFFER: u32 = 1u;
const SALT_ACCEPT: u32 = 2u;
const SALT_FLOW: u32 = 3u;
const SALT_START: u32 = 4u;
const SALT_EARN: u32 = 5u;
const SALT_FRICTION: u32 = 6u;
const TOP_SPEED: u32 = 3u;
const UP: u32 = 0u;
const UP_RIGHT: u32 = 1u;
const DOWN: u32 = 3u;
const UP_LEFT: u32 = 5u;

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

// `within`: random `bits` fall within `chance`, out of 65,536.
fn within(bits: u32, chance: u32) -> bool {
    return (bits & 0xffffu) < chance;
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

fn height(x: i32, y: i32) -> i32 {
    return -(2 * y + (x & 1));
}

// `drop` in crates/sim: how far a move toward `d` drops, in half cells.
fn fall(x: i32, y: i32, d: u32) -> i32 {
    let n = neighbor(x, y, d);
    return height(x, y) - height(n.x, n.y);
}

fn opposite(d: u32) -> u32 {
    return (d + 3u) % 6u;
}

fn material_of(s: u32) -> u32 {
    return min(s & 7u, BEDROCK);
}

fn moves(m: u32) -> bool {
    return m == SAND || m == WATER;
}

// The offer and the acceptance, as 1 + a direction, or 0 for none.
fn offer_of(s: u32) -> u32 {
    return (s >> 3u) & 7u;
}

// The offer's terms: 1 when water comes back.
fn terms_of(s: u32) -> u32 {
    return (s >> 6u) & 1u;
}

fn accept_of(s: u32) -> u32 {
    return (s >> 7u) & 7u;
}

fn rejects_of(s: u32) -> u32 {
    return (s >> 10u) & 63u;
}

fn speed_of(s: u32) -> u32 {
    return (s >> 16u) & 3u;
}

fn flow_of(s: u32) -> u32 {
    return (s >> 18u) & 1u;
}

// `State::encode`.
fn encode(m: u32, offer: u32, terms: u32, accept: u32, rejects: u32, speed: u32, flow: u32) -> u32 {
    return m | (offer << 3u) | (terms << 6u) | (accept << 7u) | ((rejects & 63u) << 10u)
        | (min(speed, TOP_SPEED) << 16u) | (flow << 18u);
}

// `Side::of` in crates/sim, as a flow bit: 1 right, 0 left, 2 for a vertical move.
fn side_of(d: u32) -> u32 {
    switch d {
        case 1u, 2u: { return 1u; }
        case 4u, 5u: { return 0u; }
        default: { return 2u; }
    }
}

// `Side::up`: the upper diagonal on a side.
fn up_of(right: u32) -> u32 {
    return select(UP_LEFT, UP_RIGHT, right == 1u);
}

// `pick` in crates/sim: the candidate with the highest key, equals chosen by `bits`.
// Returns 1 + the direction, or 0 for none.
struct Picker {
    best: array<u32, 6>,
    n: u32,
    key: i32,
}

fn picker() -> Picker {
    return Picker(array<u32, 6>(0u, 0u, 0u, 0u, 0u, 0u), 0u, -2147483647 - 1);
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

// `passes_on` in `next`: a side offer from `giver` (below) is taken only where the water could go on
// downhill from here.
fn passes_on(seen: ptr<function, array<u32, 6>>, giver: u32) -> bool {
    if giver != 2u && giver != 4u {
        return false;
    }
    return material_of((*seen)[DOWN]) == AIR || material_of((*seen)[6u - giver]) == AIR;
}

// `starts` in `next`: a free fall straight down into air goes on the start chance for its speed.
fn starts(seen: ptr<function, array<u32, 6>>, x: i32, y: i32, phase: u32, d: u32, speed: u32) -> bool {
    let free_fall = d == DOWN && material_of((*seen)[DOWN]) == AIR;
    return !free_fall || within(hash(x, y, phase, SALT_START), params.start[min(speed, TOP_SPEED)]);
}

// Matter at `d` pushes water stepping aside, unless it is moving into it: offered to it, or the
// giver it just took from (`me_accept`), still showing what is now its own.
fn pushes(seen: ptr<function, array<u32, 6>>, d: u32, me_accept: u32) -> bool {
    let n = (*seen)[d];
    return moves(material_of(n)) && offer_of(n) != opposite(d) + 1u && me_accept != d + 1u;
}

// Water stepping to `side` is pushed by matter above it, or above it on the side it leaves.
fn pushed(seen: ptr<function, array<u32, 6>>, side: u32, me_accept: u32) -> bool {
    return pushes(seen, UP, me_accept) || pushes(seen, up_of(1u - side), me_accept);
}

// One update: clear, answer, offer (`next` in crates/sim).
fn next(me: u32, kept: u32, seen: ptr<function, array<u32, 6>>, x: i32, y: i32, phase: u32) -> u32 {
    var m = material_of(me);
    var offer = 0u;
    var terms = 0u;
    var accept = 0u;
    var rejects = 0u;
    var speed = speed_of(me);
    var flow = flow_of(me);
    // The offer I published this phase is answered at the next, so its matter is promised.
    let promised = offer_of(me) != 0u;

    // 1. Clear: the offer kept back in my older word has its answer now.
    let waited = offer_of(kept);
    if waited != 0u {
        let d = waited - 1u;
        if accept_of((*seen)[d]) == opposite(d) + 1u {
            // Accepted: clear to what the terms send back; returned water arrives still, hashed flow.
            m = select(AIR, WATER, terms_of(kept) == 1u);
            speed = 0u;
            flow = 0u;
            if m == WATER && (hash(x, y, phase, SALT_FLOW) & 1u) == 1u {
                flow = 1u;
            }
        } else if m == WATER && fall(x, y, d) < 0 {
            // A side step turned down: flip.
            flow = 1u - flow;
        }
    }

    // 2. Answer every offer aimed at me: air takes a fall, or a side step it can pass on; water takes
    // sand on terms that the water goes back. Rank the rest, accept the first, reject the others.
    var p = picker();
    for (var d = 0u; d < 6u; d++) {
        let n = (*seen)[d];
        if offer_of(n) != opposite(d) + 1u || promised {
            continue;
        }
        var takes = false;
        if m == AIR && terms_of(n) == 0u {
            takes = fall(x, y, d) < 0 || passes_on(seen, d);
        } else if m == WATER && terms_of(n) == 1u {
            takes = material_of(n) == SAND;
        }
        if takes {
            consider(&p, d, -fall(x, y, d));
        }
    }
    let taken = chosen(&p, hash(x, y, phase, SALT_ACCEPT));
    for (var d = 0u; d < 6u; d++) {
        if offer_of((*seen)[d]) == opposite(d) + 1u && taken != d + 1u {
            rejects |= 1u << d;
        }
    }
    if taken != 0u {
        let d = taken - 1u;
        let n = (*seen)[d];
        // Speed travels with the matter, plus what a fall earned; a trade drags it to nothing.
        if terms_of(n) == 1u {
            speed = 0u;
        } else if fall(x, y, d) < 0 {
            let chance = select(params.gravity / 2u, params.gravity, d == UP);
            let earned = select(0u, 1u, within(hash(x, y, phase, SALT_EARN), chance));
            speed = min(speed_of(n) + earned, TOP_SPEED);
        } else {
            speed = speed_of(n);
        }
        // Water brings its flow, turned toward the side it moved to.
        let side = side_of(opposite(d));
        flow = select(0u, select(side, flow_of(n), side == 2u), material_of(n) == WATER);
        m = material_of(n);
        accept = taken;
    }

    // 3. Offer, unless my matter is promised.
    if promised {
        return encode(m, offer, terms, accept, rejects, speed, flow);
    }
    if m == SAND {
        // The steepest downhill neighbor holding air, or still water to trade with. Nowhere to go:
        // landed, and sand stops dead.
        var q = picker();
        for (var d = 0u; d < 6u; d++) {
            let n = material_of((*seen)[d]);
            if fall(x, y, d) > 0 && (n == AIR || (n == WATER && offer_of((*seen)[d]) == 0u)) {
                consider(&q, d, fall(x, y, d));
            }
        }
        let c = chosen(&q, hash(x, y, phase, SALT_OFFER));
        if c == 0u {
            speed = 0u;
        } else if starts(seen, x, y, phase, c - 1u, speed) {
            offer = c;
            terms = select(0u, 1u, material_of((*seen)[c - 1u]) == WATER);
        }
    }
    if m == WATER {
        // Fall: straight down first, then the diagonal on the flow side.
        var q = picker();
        for (var d = 0u; d < 6u; d++) {
            if fall(x, y, d) > 0 && material_of((*seen)[d]) == AIR {
                consider(&q, d, 2 * fall(x, y, d) + select(0, 1, side_of(d) == flow));
            }
        }
        let c = chosen(&q, 0u);
        if c != 0u {
            if starts(seen, x, y, phase, c - 1u, speed) {
                offer = c;
            }
        } else {
            // Landed: keep at most the splash. With sand above, wait for it to sink in. Side step:
            // the flow side first, else the other. Pushed, it is free; otherwise it needs speed, and
            // costs a level on the friction chance.
            speed = min(speed, params.splash);
            let laden = material_of((*seen)[UP]) == SAND || material_of((*seen)[UP_LEFT]) == SAND
                || material_of((*seen)[UP_RIGHT]) == SAND;
            let took = accept_of(me);
            var side = 2u;
            if !laden {
                if material_of((*seen)[up_of(flow)]) == AIR && (speed > 0u || pushed(seen, flow, took)) {
                    side = flow;
                } else if material_of((*seen)[up_of(1u - flow)]) == AIR && (speed > 0u || pushed(seen, 1u - flow, took)) {
                    side = 1u - flow;
                }
            }
            if side != 2u {
                flow = side;
                offer = up_of(side) + 1u;
                if !pushed(seen, side, took) && within(hash(x, y, phase, SALT_FRICTION), params.friction) {
                    speed -= 1u;
                }
            }
        }
    }
    return encode(m, offer, terms, accept, rejects, speed, flow);
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

// The brush (`World::paint`), between phases: every cell inside a stamp holds its material
// and nothing in flight. Matter goes only into cells holding air and no acceptance; erasing clears
// any matter but bedrock. A giver whose offer was accepted is left alone, and a cell's answers stand,
// so an erased receiver keeps its acceptance, on air, for a phase. No cell's answers change, so
// reading a neighbor's mid-brush is safe. Stamps apply in order.
@compute @workgroup_size(8, 8)
fn brush(@builtin(global_invocation_id) id: vec3<u32>) {
    let x = i32(id.x);
    let y = i32(id.y);
    if x >= params.width || y >= params.height {
        return;
    }
    for (var d = 0u; d < 6u; d++) {
        let n = neighbor(x, y, d);
        if inside(n) && accept_of(atomicLoad(&cells[current(n.y * params.width + n.x)])) == opposite(d) + 1u {
            return; // an accepted giver
        }
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
            && select(here == AIR && accept_of(state) == 0u, here != BEDROCK && here != AIR, material == AIR) {
            let right = select(0u, 1u, material == WATER && (hash(x, y, 0u, SALT_FLOW) & 1u) == 1u);
            state = encode(material, 0u, 0u, accept_of(state), rejects_of(state), 0u, right);
            painted = true;
        }
    }
    if painted {
        atomicStore(&cells[slot], state | (word & ~STATE_MASK));
    }
}
