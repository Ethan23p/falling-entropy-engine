// The sim on the GPU: a line-for-line mirror of crates/sim (`next`, `World::update`,
// `World::paint`). Keep the two identical; the GPU parity test checks them bit for bit.
//
// A cell's word: state (bits 0-10), kept-back value (bits 11-21), phase (bits 22-31).
// A state: material (bits 0-2: 0 air, 1 sand, 2 water, 3 slate, 4 bedrock), then one intent
// (bits 3-5), stored as 1 + a direction's index, 0 meaning none; then the flow (bit 6, set for
// right: water's, or the packet's for sand carrying one); then water's waited or sand's carries
// (bit 7); then whether the intent is an acceptance (bit 8); then sand's or water's speed (bits 9-10).
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

const STATE_BITS: u32 = 11u;
const STATE_MASK: u32 = 2047u;
const PHASE_SHIFT: u32 = 22u;
const PHASE_MASK: u32 = 0x3ffu;
const AIR: u32 = 0u;
const SAND: u32 = 1u;
const WATER: u32 = 2u;
const SLATE: u32 = 3u;
const BEDROCK: u32 = 4u;
const FLOW_RIGHT: u32 = 64u;
const CARRIES: u32 = 128u;
const ACCEPTING: u32 = 256u;
const SALT_OFFER: u32 = 1u;
const SALT_ACCEPT: u32 = 2u;
const SALT_FLOW: u32 = 3u;
const SALT_START: u32 = 4u;
const SALT_EARN: u32 = 5u;
const SALT_FRICTION: u32 = 6u;
const TOP_SPEED: u32 = 3u;
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

fn material_of(s: u32) -> u32 {
    return min(s & 7u, BEDROCK);
}

fn moves(m: u32) -> bool {
    return m == SAND || m == WATER;
}

// The intent, as 1 + a direction, or 0 for none: an offer, or an acceptance when bit 8 is set.
fn offer_of(s: u32) -> u32 {
    return select((s >> 3u) & 7u, 0u, (s & ACCEPTING) != 0u);
}

fn accept_of(s: u32) -> u32 {
    return select(0u, (s >> 3u) & 7u, (s & ACCEPTING) != 0u);
}

// `State::carries`: sand holding a packet of the water it displaced. 1 or 0.
fn carries(s: u32) -> u32 {
    return select(0u, 1u, material_of(s) == SAND && (s & CARRIES) != 0u);
}

// `State::sends`: the matter an offer from this cell sends.
fn sends(s: u32) -> u32 {
    return select(material_of(s), WATER, carries(s) == 1u);
}

// `State::speed`: sand's or water's, 0 to TOP_SPEED.
fn speed_of(s: u32) -> u32 {
    return select(0u, (s >> 9u) & 3u, moves(material_of(s)));
}

// A neighbor toward `d` I could follow: matter below me, itself falling straight down.
fn follows(n: u32, d: u32) -> bool {
    return d == DOWN && moves(material_of(n)) && carries(n) == 0u && offer_of(n) == DOWN + 1u;
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

// Not falling: no offer, or water's side offer (up-right or up-left).
fn unmoved(n: u32) -> bool {
    let o = offer_of(n);
    return o == 0u || o == 2u || o == 6u;
}

// A neighbor `m` could offer to: lighter matter (or air), not falling, and not accepting.
fn free(m: u32, n: u32) -> bool {
    return lighter(material_of(n), m) && unmoved(n) && accept_of(n) == 0u;
}

// An offer toward `d` stands while its target still could take it and has not accepted another.
fn standing(m: u32, n: u32, d: u32) -> bool {
    return lighter(material_of(n), m) && unmoved(n) && (accept_of(n) == 0u || accept_of(n) == opposite(d) + 1u);
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

fn next(me: u32, seen: ptr<function, array<u32, 6>>, x: i32, y: i32, phase: u32) -> u32 {
    let offer = offer_of(me);
    let accept = accept_of(me);
    var material = material_of(me);
    var new_offer = offer;
    var new_accept = accept;
    var flow = (me >> 6u) & 1u;
    var waited = (me >> 7u) & 1u;
    var carry = carries(me);
    var speed = speed_of(me);

    // Transfer, the giving half: my offer was accepted, so I hand over what I sent.
    if offer != 0u {
        let d = offer - 1u;
        let n = (*seen)[d];
        if accept_of(n) == opposite(d) + 1u {
            if carry == 1u {
                // The packet has gone; plain sand is left.
                new_offer = 0u;
                flow = 0u;
                carry = 0u;
                speed = 0u;
            } else if material_of(n) == AIR {
                // Follow: matter above offering into me takes the gap, so a column falls together.
                if d == DOWN && follows((*seen)[0], DOWN) {
                    return AIR | (1u << 3u) | ACCEPTING;
                }
                return AIR;
            } else {
                // Displacement: hold air already accepting what comes back, so nothing else falls in.
                return AIR | ((d + 1u) << 3u) | ACCEPTING;
            }
        }
    }

    // Transfer, the receiving half: the offer I accepted still stands, so I take what it sends.
    // Water brings its flow, turned toward the side it moved to.
    if accept != 0u {
        new_accept = 0u;
        let d = accept - 1u;
        let n = (*seen)[d];
        if offer_of(n) == opposite(d) + 1u {
            let sent = sends(n);
            if material == AIR {
                let side = side_of(opposite(d));
                flow = select(0u, select(side, (n >> 6u) & 1u, side == 2u), sent == WATER);
                material = sent;
                waited = 0u;
                // Speed travels with the matter, plus what a fall earned; a packet arrives still.
                if carries(n) == 1u {
                    speed = 0u;
                } else if fall(x, y, d) < 0 {
                    let chance = select(params.gravity / 2u, params.gravity, d == 0u);
                    let earned = select(0u, 1u, within(hash(x, y, phase, SALT_EARN), chance));
                    speed = min(speed_of(n) + earned, TOP_SPEED);
                } else {
                    speed = speed_of(n);
                }
            } else {
                // Displacement: take the heavier matter, and carry what I held back the way it came.
                material = sent;
                carry = 1u;
                new_offer = accept;
                waited = 0u;
                speed = 0u;
            }
        }
    }

    // Start: a free fall straight down into air goes on a chance that grows with the speed.
    // Slides, follows and sinking don't wait.
    let falls_free = within(hash(x, y, phase, SALT_START), params.start[min(speed, TOP_SPEED)])
        || material_of((*seen)[DOWN]) != AIR;

    if material == AIR {
        if accept == 0u {
            // A side offer, from below, is taken only where I could pass the water on downhill. A
            // packet is only ever offered to its own gap, which has already accepted it.
            let pass_on = material_of((*seen)[3]) == AIR;
            var p = picker();
            for (var d = 0u; d < 6u; d++) {
                let n = (*seen)[d];
                if moves(material_of(n)) && carries(n) == 0u && offer_of(n) == opposite(d) + 1u {
                    let f = fall(x, y, d);
                    var passes = false;
                    if d == 2u || d == 4u {
                        passes = pass_on || material_of((*seen)[6u - d]) == AIR;
                    }
                    if f < 0 || passes {
                        consider(&p, d, -f);
                    }
                }
            }
            new_accept = chosen(&p, hash(x, y, phase, SALT_ACCEPT));
        }
        return AIR | (new_accept << 3u) | select(0u, ACCEPTING, new_accept != 0u);
    }
    if material == SAND && carry == 1u {
        // My packet is offered back to the gap I came from, which already accepts it and which the
        // brush leaves alone, so it always goes next phase.
        return SAND | (new_offer << 3u) | (flow << 6u) | CARRIES | (speed << 9u);
    }
    if material == SAND {
        if new_offer != 0u {
            let d = new_offer - 1u;
            if !standing(SAND, (*seen)[d], d) && !follows((*seen)[d], d) {
                new_offer = 0u;
            }
        }
        if new_offer == 0u {
            var p = picker();
            for (var d = 0u; d < 6u; d++) {
                if fall(x, y, d) > 0 && (free(SAND, (*seen)[d]) || follows((*seen)[d], d)) {
                    consider(&p, d, fall(x, y, d));
                }
            }
            let c = chosen(&p, hash(x, y, phase, SALT_OFFER));
            if c == 0u {
                speed = 0u; // landed: sand stops dead
            } else if c - 1u != DOWN || falls_free {
                new_offer = c;
            }
        }
        return SAND | (new_offer << 3u) | (speed << 9u);
    }
    if material == WATER {
        if new_offer != 0u {
            let d = new_offer - 1u;
            let n = (*seen)[d];
            if !standing(WATER, n, d) && !follows(n, d) {
                new_offer = 0u;
            } else if fall(x, y, d) < 0 && accept_of(n) == 0u {
                // A side offer unanswered a second time was declined: flip, and try the other side.
                if waited == 1u {
                    new_offer = 0u;
                    flow = 1u - flow;
                } else {
                    waited = 1u;
                }
            }
        }
        if new_offer == 0u {
            waited = 0u;
            // Fall: straight down first, then the diagonal on the flow side.
            var p = picker();
            for (var d = 0u; d < 6u; d++) {
                if fall(x, y, d) > 0 && (free(WATER, (*seen)[d]) || follows((*seen)[d], d)) {
                    consider(&p, d, 2 * fall(x, y, d) + select(0, 1, side_of(d) == flow));
                }
            }
            let c = chosen(&p, 0u);
            if c != 0u {
                // About to fall, so not stepping aside.
                if c - 1u != DOWN || falls_free {
                    new_offer = c;
                }
            } else {
                // Landed: keep at most the splash. Side step: the flow side first, else the other.
                // Pushed, it is free; otherwise it needs speed, and costs a level on the friction chance.
                speed = min(speed, params.splash);
                var side = 2u;
                if free(WATER, (*seen)[up_of(flow)]) && (speed > 0u || pushed(seen, flow)) {
                    side = flow;
                } else if free(WATER, (*seen)[up_of(1u - flow)]) && (speed > 0u || pushed(seen, 1u - flow)) {
                    side = 1u - flow;
                }
                if side != 2u {
                    flow = side;
                    new_offer = up_of(side) + 1u;
                    if !pushed(seen, side) && within(hash(x, y, phase, SALT_FRICTION), params.friction) {
                        speed -= 1u;
                    }
                }
            }
        }
        if (new_offer == 0u || fall(x, y, new_offer - 1u) < 0) && accept == 0u {
            // Displacement: water that cannot fall accepts heavier matter offered from above,
            // rather than step aside.
            var p = picker();
            for (var d = 0u; d < 6u; d++) {
                let n = (*seen)[d];
                if fall(x, y, d) < 0 && offer_of(n) == opposite(d) + 1u && lighter(WATER, sends(n)) {
                    consider(&p, d, -fall(x, y, d));
                }
            }
            let a = chosen(&p, hash(x, y, phase, SALT_ACCEPT));
            if a != 0u {
                new_accept = a;
                new_offer = 0u;
                waited = 0u;
            }
        }
        let intent = select(new_offer, new_accept, new_accept != 0u);
        return WATER | (intent << 3u) | (flow << 6u) | (waited << 7u) | select(0u, ACCEPTING, new_accept != 0u)
            | (speed << 9u);
    }
    return material; // slate and bedrock never change
}

// Water stepping to `side` is pushed by matter above it, or above it on the side it leaves, that
// isn't moving into it (what it just took, as the snapshot still shows it).
fn pushed(seen: ptr<function, array<u32, 6>>, side: u32) -> bool {
    let back = up_of(1u - side);
    return pushes((*seen)[0], 0u) || pushes((*seen)[back], back);
}

fn pushes(n: u32, d: u32) -> bool {
    return moves(material_of(n)) && offer_of(n) != opposite(d) + 1u;
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
    var seen = array<u32, 6>(BEDROCK, BEDROCK, BEDROCK, BEDROCK, BEDROCK, BEDROCK);
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
// and nothing in flight. Matter goes only into cells holding air; erasing clears any matter
// but bedrock. Air that has accepted matter is left alone.
// Stamps apply in order.
@compute @workgroup_size(8, 8)
fn brush(@builtin(global_invocation_id) id: vec3<u32>) {
    let x = i32(id.x);
    let y = i32(id.y);
    if x >= params.width || y >= params.height {
        return;
    }
    let centre = vec2(f32(x), f32(y) + 0.5 * f32(x & 1));
    let i = y * params.width + x;
    let word = atomicLoad(&cells[i]);
    var state = word & STATE_MASK;
    var painted = false;
    for (var s = 0u; s < params.stamps; s++) {
        let stamp = stamps[s];
        let material = u32(stamp.material);
        let off = centre - vec2(stamp.x, stamp.y);
        if off.x * off.x + off.y * off.y <= stamp.radius * stamp.radius
            && select(material_of(state) == AIR && accept_of(state) == 0u,
                      material_of(state) != BEDROCK && material_of(state) != AIR,
                      material == AIR) {
            state = material;
            if material == WATER && (hash(x, y, 0u, SALT_FLOW) & 1u) == 1u {
                state |= FLOW_RIGHT;
            }
            painted = true;
        }
    }
    if !painted {
        return;
    }
    atomicStore(&cells[i], state | (state << STATE_BITS) | ((word >> PHASE_SHIFT) << PHASE_SHIFT));
}
