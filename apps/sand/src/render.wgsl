// The observer (the doc's View): samples the lattice into pixels, each cell a proper hexagon.
// Flat-topped hexes in columns, odd columns half a cell lower: the lattice's own topology.
//
// The render resolution is set apart from the screen: the lattice is drawn at `hex` virtual
// pixels per hexagon (corner to corner), and each virtual pixel covers `texel` screen pixels, so
// a low resolution shows chunky hexes at any screen size.

struct View {
    width: i32,
    height: i32,
    // The hexagon's size, centre to corner, in virtual pixels, and screen pixels per virtual pixel.
    size: f32,
    texel: f32,
    // The lattice's top-left on screen, in pixels.
    origin: vec2<f32>,
    // How dark hex edges draw, 0 to 1.
    edges: f32,
    // The ghost: the brush's pending stamp (`Placed`), its superhex radius (-1: none) and material.
    ghost_radius: i32,
    ghost_material: u32,
    _pad0: u32,
    _pad1: u32,
    _pad2: u32,
    // Each centre in .xy (a uniform array's stride is 16 bytes).
    ghost: array<vec4<i32>, 4>,
}

@group(0) @binding(0) var<uniform> view: View;
@group(0) @binding(1) var<storage, read> cells: array<u32>;

// Four words a cell; the first two are its state words: the state (bits 0-19) and its phase
// (bits 20-31). See sim.wgsl.
const STATE_BITS: u32 = 20u;
const PHASE_MASK: u32 = 0xfffu;
const SQRT3: f32 = 1.7320508;

// The index of cell i's current word, of the two it keeps: the one tagged with the later phase.
fn current(i: i32) -> i32 {
    let even = cells[4 * i] >> STATE_BITS;
    let odd = cells[4 * i + 1] >> STATE_BITS;
    return select(4 * i + 1, 4 * i, ((even - odd) & PHASE_MASK) == 1u);
}

const BEDROCK: vec3<f32> = vec3(0.031, 0.035, 0.047);
const AIR: vec3<f32> = vec3(0.075, 0.082, 0.106);
const SAND: vec3<f32> = vec3(0.871, 0.718, 0.431);
// Vacuum: a deep violet, darker than air and distinct from bedrock.
const VACUUM: vec3<f32> = vec3(0.098, 0.035, 0.157);
// Slate: a cool blue-grey stone; water: a clear mid blue.
const SLATE: vec3<f32> = vec3(0.322, 0.361, 0.420);
const WATER: vec3<f32> = vec3(0.165, 0.412, 0.773);

fn color(material: u32) -> vec3<f32> {
    // Codes past water read as vacuum.
    var colors = array<vec3<f32>, 7>(AIR, SAND, BEDROCK, VACUUM, SLATE, WATER, VACUUM);
    return colors[min(material, 6u)];
}

// The axial cell nearest fractional axial (q, r).
fn round_axial(q: f32, r: f32) -> vec2<i32> {
    let s = -q - r;
    var rq = round(q);
    var rr = round(r);
    let rs = round(s);
    let dq = abs(rq - q);
    let dr = abs(rr - r);
    let ds = abs(rs - s);
    if dq > dr && dq > ds {
        rq = -rr - rs;
    } else if dr > ds {
        rr = -rq - rs;
    }
    return vec2(i32(rq), i32(rr));
}

fn hex_distance(a: vec2<i32>, b: vec2<i32>) -> i32 {
    let d = a - b;
    return (abs(d.x) + abs(d.y) + abs(d.x + d.y)) / 2;
}

@vertex
fn vs(@builtin(vertex_index) i: u32) -> @builtin(position) vec4<f32> {
    // One triangle covering the screen; the scissor keeps it to the lattice's area.
    let uv = vec2(f32((i << 1u) & 2u), f32(i & 2u));
    return vec4(uv * 2.0 - 1.0, 0.0, 1.0);
}

@fragment
fn fs(@builtin(position) pos: vec4<f32>) -> @location(0) vec4<f32> {
    // The virtual pixel this screen pixel falls in, at its centre.
    let v = floor((pos.xy - view.origin) / view.texel) + 0.5;
    // Relative to cell (0, 0)'s centre, then to fractional axial coordinates (`lattice::to_axial`).
    let s = view.size;
    let p = v - vec2(s, 0.5 * SQRT3 * s);
    let a = round_axial(p.x * 2.0 / (3.0 * s), (-p.x / 3.0 + p.y * SQRT3 / 3.0) / s);
    let x = a.x;
    let y = a.y + (a.x - (a.x & 1)) / 2;
    if x < 0 || x >= view.width || y < 0 || y >= view.height {
        discard;
    }
    var c = color(cells[current(y * view.width + x)] & 7u);

    // Hex edges: darker within a virtual pixel of the hexagon's sides.
    let centre = vec2(1.5 * s * f32(a.x), SQRT3 * s * (f32(a.y) + 0.5 * f32(a.x)));
    let d = abs(p - centre);
    let inset = 0.5 * SQRT3 * s - max(d.y, 0.5 * SQRT3 * d.x + 0.5 * d.y);
    if inset < 1.0 && s >= 3.0 {
        c = c * (1.0 - view.edges);
    }

    // The ghost: the brush's pending stamp, washed toward its material and lightened.
    if view.ghost_radius >= 0 {
        var under = false;
        for (var k = 0; k < 4; k++) {
            under = under || hex_distance(a, view.ghost[k].xy) <= view.ghost_radius;
        }
        if under {
            c = mix(c, color(view.ghost_material), 0.6) + vec3(0.12);
        }
    }
    return vec4(c, 1.0);
}
