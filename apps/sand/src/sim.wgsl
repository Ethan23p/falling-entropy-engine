// The sim on the GPU: a line-for-line mirror of crates/sim (`next`, `World::update`,
// `World::paint`). Keep the two identical; the GPU parity test checks them bit for bit.
//
// A cell keeps two words, one per phase parity: word 2i + (p & 1) holds cell i's state at phase p
// (bits 0-15) and p itself (bits 16-31, wrapping). A neighbor at phase p reads that word, which the
// cell doesn't overwrite until it reaches p + 2, so cells run in any order and in place.
// A state: material (bits 0-2: 0 air, 1 sand, 2 water, 3 slate, 4 bedrock); the offer (bits 3-5)
// and the acceptance (bits 6-8), each 1 + a direction or 0 for none; the flow (bit 9, set for right);
// water's waited or sand's carries (bit 10); speed (bits 11-12). Only sand and water hold more than a
// material, but for air left by erasing a receiver, which keeps its acceptance for a phase.
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

const STATE_MASK: u32 = 0xffffu;
const PHASE_MASK: u32 = 0xffffu;
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
const DOWN: u32 = 3u;

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

fn accept_of(s: u32) -> u32 {
    return (s >> 6u) & 7u;
}

fn flow_of(s: u32) -> u32 {
    return (s >> 9u) & 1u;
}

fn flag_of(s: u32) -> u32 {
    return (s >> 10u) & 1u;
}

fn speed_of(s: u32) -> u32 {
    return (s >> 11u) & 3u;
}

// `State::carries`: sand holding a packet of the water it displaced. 1 or 0.
fn carries(s: u32) -> u32 {
    return select(0u, 1u, material_of(s) == SAND && flag_of(s) == 1u);
}

// `State::sends`: the matter an offer from this cell sends.
fn sends(s: u32) -> u32 {
    return select(material_of(s), WATER, carries(s) == 1u);
}

// `State::encode`: sand keeps a flow only while it carries a packet.
fn encode(m: u32, offer: u32, accept: u32, flow: u32, flag: u32, speed: u32) -> u32 {
    if m == AIR {
        return accept << 6u;
    }
    if !moves(m) {
        return m;
    }
    let f = select(flow, 0u, m == SAND && flag == 0u);
    return m | (offer << 3u) | (accept << 6u) | (f << 9u) | (flag << 10u) | (min(speed, TOP_SPEED) << 11u);
}

// `density` in crates/sim: air 0, water 1, sand 2; slate and bedrock never move (9).
fn density(m: u32) -> u32 {
    switch m {
        case 0u: { return 0u; }
        case 2u: { return 1u; }
        case 1u: { return 2u; }
        default: { return 9u; }
    }
}

// `lighter`: `a` is lighter than `b`, so `b` can sink into it.
fn lighter(a: u32, b: u32) -> bool {
    return density(a) < density(b) && density(b) < 9u;
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
    return select(5u, 1u, right == 1u);
}

// `open` in `next`: a neighbor `m` could offer to. Lighter matter that isn't falling (air, or water
// still or stepping aside), or matter below falling straight down (a follow).
fn open(m: u32, seen: ptr<function, array<u32, 6>>, d: u32) -> bool {
    let n = (*seen)[d];
    let o = offer_of(n);
    let still = o == 0u || o == 2u || o == 6u;
    let follow = d == DOWN && moves(material_of(n)) && carries(n) == 0u && o == DOWN + 1u;
    return (lighter(material_of(n), m) && still) || follow;
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

// `start` in `next`: a free fall straight down into air goes on the start chance for its speed.
fn starts(seen: ptr<function, array<u32, 6>>, x: i32, y: i32, phase: u32, c: u32, speed: u32) -> u32 {
    let free_fall = c == DOWN + 1u && material_of((*seen)[DOWN]) == AIR;
    return select(0u, c, !free_fall || within(hash(x, y, phase, SALT_START), params.start[min(speed, TOP_SPEED)]));
}

fn next(me: u32, seen: ptr<function, array<u32, 6>>, x: i32, y: i32, phase: u32) -> u32 {
    let me_offer = offer_of(me);
    let me_accept = accept_of(me);
    let me_carries = carries(me);
    var m = material_of(me);
    var offer = me_offer;
    var accept = 0u;
    var flow = flow_of(me);
    var flag = flag_of(me);
    var speed = speed_of(me);

    // Clear: a neighbor's acceptance names me, so what I offered is theirs.
    var taken = false;
    for (var d = 0u; d < 6u; d++) {
        if accept_of((*seen)[d]) == opposite(d) + 1u {
            taken = true;
        }
    }
    if taken {
        m = select(AIR, SAND, me_carries == 1u);
        offer = 0u;
        flow = 0u;
        flag = 0u;
        speed = 0u;
    } else if me_offer != 0u {
        // Pending: repeat a packet, or a fall whose target could still take it; else hold.
        let d = me_offer - 1u;
        let repeat = me_carries == 1u || (fall(x, y, d) > 0 && open(m, seen, d));
        offer = select(0u, me_offer, repeat);
        if m == WATER {
            flag = select(0u, 1u, fall(x, y, d) < 0);
        }
        return encode(m, offer, 0u, flow, flag, speed);
    }

    // Accept, into a cell holding nothing: a packet coming back first, then the steepest offer.
    if m == AIR {
        var got = 0u;
        for (var d = 0u; d < 6u; d++) {
            let n = (*seen)[d];
            if got == 0u && moves(material_of(n)) && offer_of(n) == opposite(d) + 1u && me_accept != d + 1u
                && carries(n) == 1u {
                got = d + 1u;
            }
        }
        if got == 0u {
            var p = picker();
            for (var d = 0u; d < 6u; d++) {
                let n = (*seen)[d];
                if moves(material_of(n)) && offer_of(n) == opposite(d) + 1u && me_accept != d + 1u {
                    var passes = false;
                    if d == 2u || d == 4u {
                        passes = material_of((*seen)[DOWN]) == AIR || material_of((*seen)[6u - d]) == AIR;
                    }
                    if fall(x, y, d) < 0 || passes {
                        consider(&p, d, -fall(x, y, d));
                    }
                }
            }
            got = chosen(&p, hash(x, y, phase, SALT_ACCEPT));
        }
        if got != 0u {
            let d = got - 1u;
            let n = (*seen)[d];
            // Speed travels with the matter, plus what a fall earned; a packet arrives still.
            if carries(n) == 1u {
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
            flow = select(select(side, flow_of(n), side == 2u), flow_of(n), carries(n) == 1u);
            if sends(n) != WATER {
                flow = 0u;
            }
            m = sends(n);
            accept = got;
            offer = 0u;
            flag = 0u;
        }
    }

    if m == SAND && flag == 0u {
        // Offer: the steepest downhill neighbor open to sand. Nowhere to go: landed, stops dead.
        var p = picker();
        for (var d = 0u; d < 6u; d++) {
            if fall(x, y, d) > 0 && open(SAND, seen, d) {
                consider(&p, d, fall(x, y, d));
            }
        }
        let c = chosen(&p, hash(x, y, phase, SALT_OFFER));
        if c == 0u {
            speed = 0u;
        } else {
            offer = starts(seen, x, y, phase, c, speed);
        }
    }
    if m == WATER {
        // Declined: my side step went unanswered, so I flip.
        if flag == 1u {
            flag = 0u;
            flow = 1u - flow;
        }
        // Fall: straight down first, then the diagonal on the flow side.
        var p = picker();
        for (var d = 0u; d < 6u; d++) {
            if fall(x, y, d) > 0 && open(WATER, seen, d) {
                consider(&p, d, 2 * fall(x, y, d) + select(0, 1, side_of(d) == flow));
            }
        }
        let c = chosen(&p, 0u);
        // Trade: water that can't fall takes heavier matter offered from above.
        var trade = 0u;
        if c == 0u && accept == 0u {
            var q = picker();
            for (var d = 0u; d < 6u; d++) {
                let n = (*seen)[d];
                if fall(x, y, d) < 0 && offer_of(n) == opposite(d) + 1u && lighter(WATER, sends(n)) {
                    consider(&q, d, -fall(x, y, d));
                }
            }
            trade = chosen(&q, hash(x, y, phase, SALT_ACCEPT));
        }
        if c != 0u {
            offer = starts(seen, x, y, phase, c, speed);
        } else if trade != 0u {
            // Hold the sand, and carry my water as a packet offered straight back to the giver.
            m = sends((*seen)[trade - 1u]);
            accept = trade;
            offer = trade;
            flag = 1u;
            speed = 0u;
        } else {
            // Landed: keep at most the splash. Side step: the flow side first, else the other.
            // Pushed, it is free; otherwise it needs speed, and costs a level on the friction chance.
            speed = min(speed, params.splash);
            var side = 2u;
            if open(WATER, seen, up_of(flow)) && (speed > 0u || pushed(seen, flow)) {
                side = flow;
            } else if open(WATER, seen, up_of(1u - flow)) && (speed > 0u || pushed(seen, 1u - flow)) {
                side = 1u - flow;
            }
            if side != 2u {
                flow = side;
                offer = up_of(side) + 1u;
                if !pushed(seen, side) && within(hash(x, y, phase, SALT_FRICTION), params.friction) {
                    speed -= 1u;
                }
            }
        }
    }
    return encode(m, offer, accept, flow, flag, speed);
}

// Water stepping to `side` is pushed by matter above it, or above it on the side it leaves, that
// isn't moving into it.
fn pushed(seen: ptr<function, array<u32, 6>>, side: u32) -> bool {
    let back = up_of(1u - side);
    return pushes((*seen)[UP], UP) || pushes((*seen)[back], back);
}

fn pushes(n: u32, d: u32) -> bool {
    return moves(material_of(n)) && offer_of(n) != opposite(d) + 1u;
}

// The index of cell i's current word: the one tagged with the later phase.
fn current(i: i32) -> i32 {
    let even = atomicLoad(&cells[2 * i]) >> 16u;
    let odd = atomicLoad(&cells[2 * i + 1]) >> 16u;
    return select(2 * i + 1, 2 * i, ((even - odd) & PHASE_MASK) == 1u);
}

// One phase for one cell (`World::update`). Cells run in any order and in place: each reads its
// neighbors' words of its own phase's parity, and writes the other.
@compute @workgroup_size(8, 8)
fn step(@builtin(global_invocation_id) id: vec3<u32>) {
    let x = i32(id.x);
    let y = i32(id.y);
    if x >= params.width || y >= params.height {
        return;
    }
    let i = y * params.width + x;
    let phase = atomicLoad(&cells[current(i)]) >> 16u;
    let parity = i32(phase & 1u);
    var seen = array<u32, 6>(BEDROCK, BEDROCK, BEDROCK, BEDROCK, BEDROCK, BEDROCK);
    for (var d = 0u; d < 6u; d++) {
        let n = neighbor(x, y, d);
        if !inside(n) {
            continue;
        }
        let nw = atomicLoad(&cells[2 * (n.y * params.width + n.x) + parity]);
        if (nw >> 16u) != phase {
            return; // a neighbor is behind
        }
        seen[d] = nw & STATE_MASK;
    }
    let me = atomicLoad(&cells[2 * i + parity]) & STATE_MASK;
    let ahead = (phase + 1u) & PHASE_MASK;
    atomicStore(&cells[2 * i + i32(ahead & 1u)], next(me, &seen, x, y, phase) | (ahead << 16u));
}

// The brush (`World::paint`), between phases: every cell inside a stamp holds its material
// and nothing in flight. Matter goes only into cells holding air; erasing clears any matter
// but bedrock. A record is left alone, and an erased receiver keeps its acceptance, on air, for a
// phase. No cell's acceptance changes, so reading a neighbor's mid-brush is safe. Stamps apply in
// order.
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
            return; // a record
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
            state = encode(material, 0u, accept_of(state), right, 0u, 0u);
            painted = true;
        }
    }
    if painted {
        atomicStore(&cells[slot], state | (word & ~STATE_MASK));
    }
}
