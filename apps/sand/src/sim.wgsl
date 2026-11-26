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
// (1 + a direction, or 0 for none), the rejections in bits 9-14 (one per direction). A velocity (`Velocity::encode`): x in the low 16 bits, y in the high, both signed.
// Outside the lattice reads as bedrock.
// Directions, clockwise from up: 0 up, 1 up-right, 2 down-right, 3 down, 4 down-left, 5 up-left.

// A material's tunable properties (`physics::Properties`).
struct Properties {
    mass: u32,
    into: u32,
    deflect: u32,
    _pad: u32,
}

// The lattice's size and the stamp count, then the physics distribution's data (`Physics`):
// gravity, the velocity ceiling, and each material's properties, indexed by its encoded value.
struct Params {
    width: i32,
    height: i32,
    stamps: u32,
    gravity: u32,
    ceiling: u32,
    _pad0: u32,
    _pad1: u32,
    _pad2: u32,
    materials: array<Properties, 6>,
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
const SALT_OFFSET: u32 = 4u;
const INTO_CAP: u32 = 255u;
const MASS_CAP: u32 = 16u;
const WEIGHT_TOP: u32 = 65536u;
const HALVINGS_CAP: u32 = 16u;
const CEILING_CAP: u32 = 8192u;
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
    // One bit per direction.
    reject: u32,
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
    return State(material, velocity, offer, accept, (bits >> 9u) & 63u);
}

fn encode(s: State) -> u32 {
    return s.material | (s.offer << 3u) | (s.accept << 6u) | ((s.reject & 63u) << 9u);
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
    return State(m, v, 0u, 0u, 0u);
}

// Particle (`particle`): `Material::is_particle`, `is_substantial`, `is_static`, `moves`.

fn is_particle(m: u32) -> bool {
    return m != VACUUM;
}

fn is_static(m: u32) -> bool {
    return m == SLATE || m == BEDROCK;
}

// Air is insubstantial: not conserved, and a more substantial particle may transfer into it.
fn is_substantial(m: u32) -> bool {
    return is_particle(m) && m != AIR;
}

fn moves(m: u32) -> bool {
    return is_particle(m) && !is_static(m);
}

// `Velocity::along`.
fn along(v: vec2<i32>, d: u32) -> i32 {
    let s = step_of(d);
    return v.x * s.x + v.y * s.y;
}

// The physics distribution (`physics`): constants and pure functions only.

fn mass(m: u32) -> u32 {
    return min(params.materials[m].mass, MASS_CAP);
}

// `Physics::into`: how amenable a particle is to offering into a cell holding dest.
fn into(dest: u32) -> u32 {
    return min(params.materials[dest].into, INTO_CAP);
}

// `Physics::deflection`.
fn deflection(m: u32) -> u32 {
    return min(params.materials[m].deflect, DEFLECT_CAP);
}

// `Physics::offers_to`, propose's exclusion: only the physically impossible. A particle that
// doesn't move offers nothing, and nothing offers into a static particle.
fn offers_to(m: u32, dest: u32) -> bool {
    return moves(m) && !is_static(dest) && into(dest) > 0u;
}

// `Physics::air_meets_vacuum`: a cell holding air with vacuum in its neighborhood offers only into
// the vacuum.
fn air_meets_vacuum(m: u32, seen: ptr<function, array<State, 6>>) -> bool {
    var found = false;
    for (var d = 0u; d < 6u; d++) {
        found = found || (*seen)[d].material == VACUUM;
    }
    return m == AIR && found;
}

// `Physics::entertains`, evaluate's exclusion, the capacity limit: a cell holding a substantial
// particle rejects every offer of one; holding nothing, it can take any; holding air, a substantial
// one, and the air ceases to exist.
fn entertains(offered: u32, dest: u32, committed: bool) -> bool {
    let room = !is_particle(dest) || (!is_substantial(dest) && is_substantial(offered) && !committed);
    return moves(offered) && room;
}

// `physics::isqrt`.
fn isqrt(n0: u32) -> u32 {
    var n = n0;
    var r = 0u;
    var bit = 1u << 30u;
    while bit > n {
        bit = bit >> 2u;
    }
    while bit != 0u {
        if n >= r + bit {
            n -= r + bit;
            r = (r >> 1u) + bit;
        } else {
            r = r >> 1u;
        }
        bit = bit >> 2u;
    }
    return r;
}

// `Physics::preference`: halving with the turn from the velocity, as many times as mass and
// magnitude say.
fn preference(m: u32, v: vec2<i32>, d: u32) -> u32 {
    let s = step_of(d);
    let magnitude = isqrt(u32(v.x * v.x + v.y * v.y));
    let norm = i32(isqrt(u32((v.x * v.x + v.y * v.y) * (s.x * s.x + s.y * s.y))));
    var turn = 1024u;
    if norm != 0 {
        turn = u32(clamp(1024 - along(v, d) * 1024 / norm, 0, 2048));
    }
    let halvings = (mass(m) * min(magnitude, CEILING_CAP) * turn) >> 8u;
    let whole = halvings >> 10u;
    if whole >= HALVINGS_CAP {
        return 0u;
    }
    let w = WEIGHT_TOP >> whole;
    return w - ((w * (halvings & 1023u)) >> 11u);
}

// `Physics::weight`.
fn weight(m: u32, v: vec2<i32>, d: u32, dest: u32) -> u32 {
    return into(dest) * mass(m) * preference(m, v, d);
}

// `Physics::moved`, gravity: its impulse, added on a move.
fn moved(m: u32, v: vec2<i32>) -> vec2<i32> {
    let impulse = i32(min(params.gravity, 4096u) * mass(m));
    return impelled(v, vec2(0, impulse));
}

// `Physics::impelled`: v with the impulse added, its gain in magnitude eased toward the ceiling.
fn impelled(v: vec2<i32>, impulse: vec2<i32>) -> vec2<i32> {
    let c = i32(clamp(params.ceiling, 1u, CEILING_CAP));
    let cap = i32(CEILING_CAP);
    let u = v + clamp(impulse, vec2(-cap), vec2(cap));
    let before = min(i32(isqrt(u32(v.x * v.x + v.y * v.y))), c);
    let after = i32(isqrt(u32(u.x * u.x + u.y * u.y)));
    if after == 0 {
        return vec2(0, SCALE);
    }
    var eased = after;
    if after > before {
        eased = before + (after - before) * (c - before) / c;
    }
    return vec2(u.x * eased / after, u.y * eased / after);
}

// Contract (`contract`): an offer's terms pair each condition (the verdict) with an effect on each
// end. Accepted: the origin releases (air spawns a new air), the target receives. Rejected: the
// origin deflects, the target does nothing.

// `Offer::verdict`: an offer toward d is accepted if the target's acceptance names this cell,
// rejected if its rejections do.
fn accepted(d: u32, dest: State) -> bool {
    return dest.accept == opposite(d) + 1u;
}

fn rejected(d: u32, dest: State) -> bool {
    return ((dest.reject >> opposite(d)) & 1u) == 1u;
}

// `Effect::Release`: the particle leaves; the cell holds nothing.
fn release(live: ptr<function, State>) {
    *live = holding(VACUUM);
}

// `Effect::Spawn`: the air leaves, and the cell spawns itself a fresh one.
fn spawn(live: ptr<function, State>) {
    *live = holding((*live).material);
}

// `Effect::Receive`: the particle arrives from the origin toward d, with gravity's impulse, and the
// verdict names that origin.
fn receive(live: ptr<function, State>, d: u32, origin: State) {
    (*live).material = origin.material;
    (*live).velocity = moved(origin.material, origin.velocity);
    (*live).accept = d + 1u;
}

// `Effect::Deflect`: the particle stays, its intent turned away from the target toward d by its
// material's deflection.
fn deflect(live: ptr<function, State>, d: u32, bits: u32) {
    let v = (*live).velocity;
    (*live).velocity = impelled(v, deflection_of(i32(deflection((*live).material)), v, d, bits));
}

// `contract::deflection`: the impulse on v rejected toward d, k perpendicular to d, to the side it
// already leans (or the side bits picks when dead on).
fn deflection_of(k: i32, v: vec2<i32>, d: u32, bits: u32) -> vec2<i32> {
    if k == 0 {
        return vec2(0, 0);
    }
    let s = step_of(d);
    let lean = -v.x * s.y + v.y * s.x;
    var side = -1;
    if lean > 0 || (lean == 0 && (bits & 1u) == 1u) {
        side = 1;
    }
    return vec2(-side * k * s.y / 2, side * k * s.x / 2);
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

// `update::offers_now`: only the very first phase is in question: on initialization, whether a cell
// offers at once or waits one phase is a coin flip.
fn offers_now(x: i32, y: i32, phase: u32) -> bool {
    return phase != 0u || (hash(x, y, 0u, SALT_OFFSET) & 1u) == 0u;
}

// One update: read, resolve, evaluate, evolve, publish, propose (`update::next`).
fn next(me: State, kept: State, seen: ptr<function, array<State, 6>>, x: i32, y: i32, phase: u32) -> State {
    var live = me;
    live.offer = 0u;
    live.accept = 0u;
    live.reject = 0u;
    var standing = false;

    // Resolve: the offer kept back, with its verdict if it has one: apply the contract's effect on
    // its origin. With none yet, it stands: publish it again.
    if kept.offer != 0u {
        let d = kept.offer - 1u;
        if accepted(d, (*seen)[d]) {
            if is_substantial(kept.material) {
                release(&live);
            } else {
                spawn(&live);
            }
        } else if rejected(d, (*seen)[d]) {
            deflect(&live, d, hash(x, y, phase, SALT_DEFLECT));
        } else {
            live.offer = kept.offer;
            standing = true;
        }
    }
    // An offer out, or standing, ties up my particle.
    let committed = me.offer != 0u || standing;

    // Evaluate: the offers that target me, excluded unless the physics distribution entertains them,
    // the rest weighted; the one selected is accepted. Holding one particle at most, I then exclude
    // the rest. Every offer evaluated gets a verdict: accepted, or rejected.
    {
        var weights = array<u32, 6>(0u, 0u, 0u, 0u, 0u, 0u);
        var offered = 0u;
        for (var d = 0u; d < 6u; d++) {
            let origin = (*seen)[d];
            if origin.offer == opposite(d) + 1u {
                offered |= 1u << d;
                if entertains(origin.material, live.material, committed) {
                    weights[d] = max(weight(origin.material, origin.velocity, opposite(d), live.material), 1u);
                }
            }
        }
        let i = select_from(&weights, hash(x, y, phase, SALT_EVALUATE));
        var taken = 0u;
        if i != 0u {
            taken = 1u << (i - 1u);
            receive(&live, i - 1u, (*seen)[i - 1u]);
        }
        live.reject = offered & ~taken;
    }

    // Evolve: no natural contracts yet.

    // Publish, then propose, in one write (see `update::next`).
    // Propose: from the six neighbors, exclude those it wouldn't offer to, select among the rest,
    // weighted.
    if !committed && offers_now(x, y, phase) {
        var weights = array<u32, 6>(0u, 0u, 0u, 0u, 0u, 0u);
        let only_vacuum = air_meets_vacuum(live.material, seen);
        for (var d = 0u; d < 6u; d++) {
            let dest = (*seen)[d].material;
            if only_vacuum && dest != VACUUM {
                continue;
            }
            if offers_to(live.material, dest) {
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
