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

const OUTSIDE: vec3<f32> = vec3(0.031, 0.035, 0.047);
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
        return vec4(OUTSIDE, 1.0);
    }
    let sand = (cells[y * view.width + x] & 1u) == 1u;
    return vec4(select(AIR, SAND, sand), 1.0);
}
