//! The lattice on the GPU: the cell buffer, the sim and brush compute passes, and the
//! render pass that samples it. The shaders mirror `crates/sim`.

use std::sync::Arc;
use std::sync::atomic::{AtomicU8, Ordering};

use engine::wgpu;

/// The most stamps one brush pass applies.
pub const MAX_STAMPS: usize = 256;
/// The sim's uniforms: four words, then four per material (`Params` in sim.wgsl).
const PARAMS_BYTES: u64 = 4 * (8 + 4 * 6);

/// A brush stamp: a shape and the material it paints (`World::stamp`, `World::stamp_tetrahex`).
#[derive(Clone, Copy, Debug)]
pub struct Stamp {
    pub shape: Shape,
    pub material: sim::Material,
}

#[derive(Clone, Copy, Debug)]
pub enum Shape {
    /// A disc of cell centres, in cell units: `y` already includes odd columns' half-cell shift.
    /// Scene scripts stamp these.
    Disc { x: f32, y: f32, radius: f32 },
    /// The player's brush.
    Tetrahex(sim::Placed),
}

/// Bytes per stamp (`Stamp` in sim.wgsl).
const STAMP_BYTES: usize = 64;
/// The render shader's uniforms (`View` in render.wgsl).
const VIEW_BYTES: u64 = 112;

impl Stamp {
    fn words(&self) -> [u32; STAMP_BYTES / 4] {
        let mut w = [0u32; STAMP_BYTES / 4];
        w[1] = self.material as u32;
        match self.shape {
            Shape::Disc { x, y, radius } => {
                w[2] = radius.to_bits();
                w[4] = x.to_bits();
                w[5] = y.to_bits();
            }
            Shape::Tetrahex(t) => {
                w[0] = 1;
                w[3] = t.radius as u32;
                for (k, (q, r)) in t.centres.iter().enumerate() {
                    w[8 + 2 * k] = *q as u32;
                    w[9 + 2 * k] = *r as u32;
                }
            }
        }
        w
    }

    /// Applies this stamp to the CPU reference, as the brush pass does on the GPU.
    #[cfg(test)]
    pub fn apply(&self, world: &mut sim::World) {
        match self.shape {
            Shape::Disc { x, y, radius } => world.stamp(x, y, radius, self.material),
            Shape::Tetrahex(t) => world.stamp_tetrahex(&t, self.material),
        }
    }
}

/// Pipelines: made once per device and target format.
pub struct Pipelines {
    step: wgpu::ComputePipeline,
    brush: wgpu::ComputePipeline,
    settle: wgpu::ComputePipeline,
    render: wgpu::RenderPipeline,
}

impl Pipelines {
    pub fn new(device: &wgpu::Device, format: wgpu::TextureFormat) -> Self {
        let sim = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("sim"),
            source: wgpu::ShaderSource::Wgsl(include_str!("sim.wgsl").into()),
        });
        let compute = |entry: &str| {
            device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                label: Some(entry),
                layout: None,
                module: &sim,
                entry_point: Some(entry),
                compilation_options: Default::default(),
                cache: None,
            })
        };
        let render = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("render"),
            source: wgpu::ShaderSource::Wgsl(include_str!("render.wgsl").into()),
        });
        let render = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("render"),
            layout: None,
            vertex: wgpu::VertexState {
                module: &render,
                entry_point: Some("vs"),
                compilation_options: Default::default(),
                buffers: &[],
            },
            primitive: Default::default(),
            depth_stencil: None,
            multisample: Default::default(),
            fragment: Some(wgpu::FragmentState {
                module: &render,
                entry_point: Some("fs"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState { format, blend: None, write_mask: wgpu::ColorWrites::ALL })],
            }),
            multiview_mask: None,
            cache: None,
        });
        Self { step: compute("step"), brush: compute("brush"), settle: compute("settle"), render }
    }
}

/// One lattice's buffers and bind groups.
pub struct Lattice {
    pub width: i32,
    pub height: i32,
    /// Read back by the GPU parity test.
    #[cfg_attr(not(test), allow(dead_code))]
    pub cells: wgpu::Buffer,
    params: wgpu::Buffer,
    stamps: wgpu::Buffer,
    view: wgpu::Buffer,
    /// The brush's offers waiting on each cell (`World::brush`).
    #[allow(dead_code)]
    offers: wgpu::Buffer,
    step_group: wgpu::BindGroup,
    brush_group: wgpu::BindGroup,
    settle_group: wgpu::BindGroup,
    render_group: wgpu::BindGroup,
}

fn words(values: &[u32]) -> Vec<u8> {
    values.iter().flat_map(|v| v.to_le_bytes()).collect()
}

fn floats(values: &[f32]) -> Vec<u8> {
    values.iter().flat_map(|v| v.to_le_bytes()).collect()
}

impl Lattice {
    /// Uploads `world` as the starting lattice.
    pub fn new(device: &wgpu::Device, pipelines: &Pipelines, world: &sim::World) -> Self {
        use wgpu::util::DeviceExt;
        let (width, height) = (world.width(), world.height());
        let cells = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("cells"),
            contents: &words(world.words()),
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC | wgpu::BufferUsages::COPY_DST,
        });
        let uniform = |label, size| {
            device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(label),
                size,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            })
        };
        let params = uniform("params", PARAMS_BYTES);
        let view = uniform("view", VIEW_BYTES);
        let stamps = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("stamps"),
            size: (MAX_STAMPS * STAMP_BYTES) as u64,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let offers = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("brush offers"),
            size: (width * height * 4) as u64,
            usage: wgpu::BufferUsages::STORAGE,
            mapped_at_creation: true,
        });
        // Mapped at creation, so it starts zeroed: no offers waiting.
        offers.unmap();
        let group = |label, layout: wgpu::BindGroupLayout, entries: &[(u32, &wgpu::Buffer)]| {
            let entries: Vec<_> = entries
                .iter()
                .map(|&(binding, buffer)| wgpu::BindGroupEntry { binding, resource: buffer.as_entire_binding() })
                .collect();
            device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some(label),
                layout: &layout,
                entries: &entries,
            })
        };
        let step_group = group("step", pipelines.step.get_bind_group_layout(0), &[(0, &params), (1, &cells)]);
        let brush_group =
            group("brush", pipelines.brush.get_bind_group_layout(0), &[(0, &params), (2, &stamps), (3, &offers)]);
        let settle_group =
            group("settle", pipelines.settle.get_bind_group_layout(0), &[(0, &params), (1, &cells), (3, &offers)]);
        let render_group = group("render", pipelines.render.get_bind_group_layout(0), &[(0, &view), (1, &cells)]);
        Self { width, height, cells, params, stamps, view, offers, step_group, brush_group, settle_group, render_group }
    }

    fn groups(&self) -> (u32, u32) {
        ((self.width as u32).div_ceil(8), (self.height as u32).div_ceil(8))
    }

    /// Applies brush stamps, then runs `phases` phases, each one dispatch.
    pub fn simulate(
        &self,
        queue: &wgpu::Queue,
        pipelines: &Pipelines,
        encoder: &mut wgpu::CommandEncoder,
        stamps: &[Stamp],
        physics: &sim::Physics,
        phases: u32,
    ) {
        let stamps = &stamps[stamps.len().saturating_sub(MAX_STAMPS)..];
        let p = physics;
        // The lattice's size, the stamp count, gravity, the velocity ceiling and the request odds (padded), then each material's
        // properties (mass, into, deflect, padding), in encoded order.
        let mut params = vec![
            self.width as u32,
            self.height as u32,
            stamps.len() as u32,
            p.gravity,
            p.ceiling,
            p.request_odds,
            0,
            0,
        ];
        for m in &p.materials {
            params.extend([m.mass, m.into, m.deflect, 0]);
        }
        queue.write_buffer(&self.params, 0, &words(&params));
        let (gx, gy) = self.groups();
        if !stamps.is_empty() {
            let data: Vec<u32> = stamps.iter().flat_map(|s| s.words()).collect();
            queue.write_buffer(&self.stamps, 0, &words(&data));
            let mut pass = encoder
                .begin_compute_pass(&wgpu::ComputePassDescriptor { label: Some("brush"), timestamp_writes: None });
            pass.set_pipeline(&pipelines.brush);
            pass.set_bind_group(0, &self.brush_group, &[]);
            pass.dispatch_workgroups(gx, gy, 1);
        }
        // Each phase is its own pass, so the next one starts after every cell has advanced; before
        // it, the brush's waiting offers settle (`World::step`).
        for _ in 0..phases {
            {
                let mut pass = encoder
                    .begin_compute_pass(&wgpu::ComputePassDescriptor { label: Some("settle"), timestamp_writes: None });
                pass.set_pipeline(&pipelines.settle);
                pass.set_bind_group(0, &self.settle_group, &[]);
                pass.dispatch_workgroups(gx, gy, 1);
            }
            let mut pass = encoder
                .begin_compute_pass(&wgpu::ComputePassDescriptor { label: Some("step"), timestamp_writes: None });
            pass.set_pipeline(&pipelines.step);
            pass.set_bind_group(0, &self.step_group, &[]);
            pass.dispatch_workgroups(gx, gy, 1);
        }
    }

    /// Draws the lattice into `target` as `look` says, inside `clip` (x, y, width, height, in
    /// pixels, already within the target). `clear` clears the whole target first.
    #[allow(clippy::too_many_arguments)]
    pub fn render(
        &self,
        queue: &wgpu::Queue,
        pipelines: &Pipelines,
        encoder: &mut wgpu::CommandEncoder,
        target: &wgpu::TextureView,
        look: &Look,
        clip: [u32; 4],
        clear: Option<wgpu::Color>,
    ) {
        let mut data = words(&[self.width as u32, self.height as u32]);
        data.extend(floats(&[look.size, look.texel, look.origin[0], look.origin[1], look.edges]));
        let (radius, material, centres) = match look.ghost {
            Some((t, m)) => (t.radius, m as u32, t.centres),
            None => (-1, 0, [(0, 0); 4]),
        };
        data.extend(words(&[radius as u32, material, look.bricks as u32, 0, 0]));
        for (q, r) in centres {
            data.extend(words(&[q as u32, r as u32, 0, 0]));
        }
        debug_assert_eq!(data.len() as u64, VIEW_BYTES);
        queue.write_buffer(&self.view, 0, &data);
        let load = match clear {
            Some(c) => wgpu::LoadOp::Clear(c),
            None => wgpu::LoadOp::Load,
        };
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("lattice"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: target,
                resolve_target: None,
                depth_slice: None,
                ops: wgpu::Operations { load, store: wgpu::StoreOp::Store },
            })],
            ..Default::default()
        });
        if clip[2] == 0 || clip[3] == 0 {
            return;
        }
        pass.set_scissor_rect(clip[0], clip[1], clip[2], clip[3]);
        pass.set_pipeline(&pipelines.render);
        pass.set_bind_group(0, &self.render_group, &[]);
        pass.draw(0..3, 0..1);
    }
}

/// How a lattice is drawn: its hexagons' size and placement, and the brush's ghost.
#[derive(Clone, Copy, Debug)]
pub struct Look {
    /// A hexagon's size, centre to corner, in virtual pixels.
    pub size: f32,
    /// Screen pixels per virtual pixel.
    pub texel: f32,
    /// The lattice's top-left on screen, in pixels.
    pub origin: [f32; 2],
    /// How dark hex edges draw, 0 to 1.
    pub edges: f32,
    /// Cells drawn as square bricks rather than hexagons.
    pub bricks: bool,
    pub ghost: Option<(sim::Placed, sim::Material)>,
}

/// The cells read back without stalling a frame: copy in one frame, map in the next, read once
/// the map lands. One read in flight at a time.
pub struct Readback {
    buffer: wgpu::Buffer,
    pub width: i32,
    pub height: i32,
    stage: Stage,
    /// Set by the map callback: 1 once mapped, 2 if mapping failed.
    mapped: Arc<AtomicU8>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Stage {
    Idle,
    /// The copy is encoded; it is submitted at the end of the frame.
    Copied,
    Mapping,
}

impl Readback {
    pub fn new(device: &wgpu::Device, lattice: &Lattice) -> Self {
        let buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("readback"),
            size: (lattice.width * lattice.height * 4 * sim::WORDS as i32) as u64,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let (width, height) = (lattice.width, lattice.height);
        Self { buffer, width, height, stage: Stage::Idle, mapped: Arc::new(AtomicU8::new(0)) }
    }

    pub fn idle(&self) -> bool {
        self.stage == Stage::Idle
    }

    /// Whether this reads `lattice`'s size of buffer.
    pub fn fits(&self, lattice: &Lattice) -> bool {
        (self.width, self.height) == (lattice.width, lattice.height)
    }

    /// Copies the cells as they will be at the end of `encoder`. Only when idle and fitting.
    pub fn start(&mut self, encoder: &mut wgpu::CommandEncoder, lattice: &Lattice) {
        debug_assert!(self.idle() && self.fits(lattice));
        encoder.copy_buffer_to_buffer(&lattice.cells, 0, &self.buffer, 0, self.buffer.size());
        self.stage = Stage::Copied;
    }

    /// Moves a read along; call once a frame. Returns the cells once they have arrived.
    pub fn poll(&mut self, device: &wgpu::Device) -> Option<Vec<u32>> {
        match self.stage {
            Stage::Idle => None,
            Stage::Copied => {
                self.mapped.store(0, Ordering::Release);
                let mapped = self.mapped.clone();
                self.buffer.slice(..).map_async(wgpu::MapMode::Read, move |r| {
                    mapped.store(if r.is_ok() { 1 } else { 2 }, Ordering::Release)
                });
                self.stage = Stage::Mapping;
                None
            }
            Stage::Mapping => {
                // Natively, map callbacks run only when the device is polled; on the web, by themselves.
                let _ = device.poll(wgpu::PollType::Poll);
                match self.mapped.load(Ordering::Acquire) {
                    0 => None,
                    1 => {
                        let words = self
                            .buffer
                            .slice(..)
                            .get_mapped_range()
                            .ok()
                            .map(|bytes| bytes.as_chunks::<4>().0.iter().map(|&b| u32::from_le_bytes(b)).collect());
                        self.buffer.unmap();
                        self.stage = Stage::Idle;
                        words
                    }
                    _ => {
                        self.stage = Stage::Idle;
                        None
                    }
                }
            }
        }
    }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use super::*;
    use sim::{Material, World};

    /// The GPU sim matches the CPU reference bit for bit. Needs a GPU, so it is skipped by
    /// default: `cargo test -p sand -- --ignored`.
    #[test]
    #[ignore]
    fn verification_gpu_matches_reference() {
        let instance = wgpu::Instance::default();
        let adapter = pollster::block_on(instance.request_adapter(&Default::default())).expect("adapter");
        let (device, queue) = pollster::block_on(adapter.request_device(&Default::default())).expect("device");
        let pipelines = Pipelines::new(&device, wgpu::TextureFormat::Rgba8Unorm);

        let (w, h) = (61, 47);
        let mut world = World::new(w, h);
        for x in 8..13 {
            world.paint(x, 10, Material::Bedrock);
        }
        for y in 0..h * 2 / 3 {
            for x in 0..w {
                if sim::hash(x, y, 0, 77).is_multiple_of(3) {
                    world.paint(x, y, Material::Sand);
                }
                if sim::hash(x, y, 0, 78).is_multiple_of(9) {
                    world.paint(x, y, Material::Air);
                }
                if sim::hash(x, y, 0, 79).is_multiple_of(5) {
                    world.paint(x, y, Material::Water);
                }
            }
        }
        let lattice = Lattice::new(&device, &pipelines, &world);
        let readback = device.create_buffer(&wgpu::BufferDescriptor {
            label: None,
            size: (w * h * 4 * sim::WORDS as i32) as u64,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });

        let mut phase = 0;
        for chunk in [1, 1, 1, 7, 40, 150] {
            let mut encoder = device.create_command_encoder(&Default::default());
            lattice.simulate(&queue, &pipelines, &mut encoder, &[], &world.physics, chunk);
            encoder.copy_buffer_to_buffer(&lattice.cells, 0, &readback, 0, (w * h * 4 * sim::WORDS as i32) as u64);
            queue.submit([encoder.finish()]);
            for _ in 0..chunk {
                world.step();
            }
            phase += chunk;

            let slice = readback.slice(..);
            slice.map_async(wgpu::MapMode::Read, |r| r.expect("map"));
            device.poll(wgpu::PollType::wait_indefinitely()).expect("poll");
            let gpu: Vec<u32> = slice
                .get_mapped_range()
                .expect("mapped")
                .as_chunks::<4>()
                .0
                .iter()
                .map(|&b| u32::from_le_bytes(b))
                .collect();
            readback.unmap();
            if let Some(report) = sim::debug::diff(&world, &World::from_words(w, h, gpu)) {
                panic!("after {phase} phases, the GPU differs from the reference: {report}");
            }
        }

        // The brush: a particle only into a cell holding nothing, erasing anything but bedrock, in order.
        let disc = |x, y, radius, material| Stamp { shape: Shape::Disc { x, y, radius }, material };
        let hex = |seed, radius, at, material| Stamp {
            shape: Shape::Tetrahex(sim::Placed::random(seed, radius, at)),
            material,
        };
        let stamps = [
            disc(20.0, 40.0, 6.5, Material::Sand),
            disc(30.0, 10.0, 8.5, Material::Sand),
            disc(22.0, 40.0, 3.5, Material::Vacuum),
            disc(10.0, 10.0, 3.5, Material::Air),
            disc(40.0, 30.0, 4.5, Material::Vacuum),
            hex(1, 2, (50, 8), Material::Sand),
            disc(12.0, 30.0, 3.5, Material::Water),
            disc(45.0, 40.0, 2.5, Material::Slate),
            hex(2, 3, (8, 20), Material::Water),
            hex(3, 1, (60, 46), Material::Slate),
            hex(4, 0, (33, 33), Material::Vacuum),
            hex(5, 2, (0, 0), Material::Sand),
        ];
        for s in &stamps {
            s.apply(&mut world);
        }
        for _ in 0..30 {
            world.step();
        }
        let mut encoder = device.create_command_encoder(&Default::default());
        lattice.simulate(&queue, &pipelines, &mut encoder, &stamps, &world.physics, 30);
        encoder.copy_buffer_to_buffer(&lattice.cells, 0, &readback, 0, (w * h * 4 * sim::WORDS as i32) as u64);
        queue.submit([encoder.finish()]);
        let slice = readback.slice(..);
        slice.map_async(wgpu::MapMode::Read, |r| r.expect("map"));
        device.poll(wgpu::PollType::wait_indefinitely()).expect("poll");
        let gpu: Vec<u32> = slice
            .get_mapped_range()
            .expect("mapped")
            .as_chunks::<4>()
            .0
            .iter()
            .map(|&b| u32::from_le_bytes(b))
            .collect();
        if let Some(report) = sim::debug::diff(&world, &World::from_words(w, h, gpu)) {
            panic!("after the brush, the GPU differs from the reference: {report}");
        }
    }
}
