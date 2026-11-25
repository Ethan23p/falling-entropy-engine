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

// Four words a cell; the first two are its state words: the state (bits 0-18) and its phase
// (bits 19-31). See sim.wgsl.
const STATE_BITS: u32 = 19u;
const PHASE_MASK: u32 = 0x1fffu;

// The index of cell i's current word, of the two it keeps: the one tagged with the later phase.
fn current(i: i32) -> i32 {
    let even = cells[4 * i] >> STATE_BITS;
    let odd = cells[4 * i + 1] >> STATE_BITS;
    return select(4 * i + 1, 4 * i, ((even - odd) & PHASE_MASK) == 1u);
}

// Outside the lattice is bedrock, so it draws as bedrock.
const BEDROCK: vec3<f32> = vec3(0.031, 0.035, 0.047);
const AIR: vec3<f32> = vec3(0.075, 0.082, 0.106);
const SAND: vec3<f32> = vec3(0.871, 0.718, 0.431);

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
    let material = min(cells[current(y * view.width + x)] & 3u, 2u);
    var colors = array<vec3<f32>, 3>(AIR, SAND, BEDROCK);
    return vec4(colors[material], 1.0);
}
