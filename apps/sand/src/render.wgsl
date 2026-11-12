// The observer: samples the lattice into pixels. Cells draw as bricks: odd columns sit
// half a cell lower, which is the column-offset hexagonal lattice's topology.

struct View {
    width: i32,
    height: i32,
    // Top-left of the lattice and the size of one cell, in pixels.
    origin: vec2<f32>,
    cell: vec2<f32>,
    _pad: vec2<f32>,
}

@group(0) @binding(0) var<uniform> view: View;
@group(0) @binding(1) var<storage, read> cells: array<u32>;

// A word: the state (bits 0-18) and its phase (bits 19-31); see sim.wgsl.
const STATE_BITS: u32 = 19u;
const STATE_MASK: u32 = 0x7ffffu;
const PHASE_MASK: u32 = 0x1fffu;

// The index of cell i's current word, of the two it keeps: the one tagged with the later phase.
// The other is its older word, holding the offer it made the phase before.
fn current(i: i32) -> i32 {
    let even = cells[2 * i] >> STATE_BITS;
    let odd = cells[2 * i + 1] >> STATE_BITS;
    return select(2 * i + 1, 2 * i, ((even - odd) & PHASE_MASK) == 1u);
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

// Outside the lattice is bedrock, so it draws as bedrock.
const BEDROCK: vec3<f32> = vec3(0.031, 0.035, 0.047);
const AIR: vec3<f32> = vec3(0.075, 0.082, 0.106);
const SAND: vec3<f32> = vec3(0.871, 0.718, 0.431);
const WATER: vec3<f32> = vec3(0.220, 0.470, 0.800);
const SLATE: vec3<f32> = vec3(0.310, 0.345, 0.400);

@vertex
fn vs(@builtin(vertex_index) i: u32) -> @builtin(position) vec4<f32> {
    // One triangle covering the screen.
    let uv = vec2(f32((i << 1u) & 2u), f32(i & 2u));
    return vec4(uv * 2.0 - 1.0, 0.0, 1.0);
}

@fragment
fn fs(@builtin(position) pos: vec4<f32>) -> @location(0) vec4<f32> {
    let p = (pos.xy - view.origin) / view.cell;
    let x = i32(floor(p.x));
    let y = i32(floor(p.y - 0.5 * f32(x & 1)));
    if p.x < 0.0 || x >= view.width || y < 0 || y >= view.height {
        return vec4(BEDROCK, 1.0);
    }
    // `World::looks`: a giver whose offer a neighbor's acceptance names draws as what the terms of
    // that offer, kept back in its older word, send back: air, or the water coming back from a trade.
    let word = current(y * view.width + x);
    var material = min(cells[word] & 7u, 4u);
    for (var d = 0u; d < 6u; d++) {
        let n = neighbor(x, y, d);
        if n.x < 0 || n.y < 0 || n.x >= view.width || n.y >= view.height {
            continue;
        }
        let t = cells[current(n.y * view.width + n.x)] & STATE_MASK;
        if ((t >> 7u) & 7u) == (d + 3u) % 6u + 1u {
            material = select(0u, 2u, ((cells[word ^ 1] >> 6u) & 1u) == 1u);
        }
    }
    var colors = array<vec3<f32>, 5>(AIR, SAND, WATER, SLATE, BEDROCK);
    return vec4(colors[material], 1.0);
}
