//! The lattice on the GPU: the cell buffer, the sim and brush compute passes, and the
//! render pass that samples it. The shaders mirror `crates/sim`.

use engine::wgpu;

/// The most stamps one brush pass applies.
pub const MAX_STAMPS: usize = 256;

/// A brush stamp, in cell units: `y` already includes odd columns' half-cell shift.
#[derive(Clone, Copy, Debug)]
pub struct Stamp {
    pub x: f32,
    pub y: f32,
    pub radius: f32,
    pub sand: bool,
}

/// Pipelines: made once per device and target format.
pub struct Pipelines {
    step: wgpu::ComputePipeline,
    brush: wgpu::ComputePipeline,
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
        Self { step: compute("step"), brush: compute("brush"), render }
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
    step_group: wgpu::BindGroup,
    brush_group: wgpu::BindGroup,
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
        let params = uniform("params", 16);
        let view = uniform("view", 32);
        let stamps = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("stamps"),
            size: (MAX_STAMPS * 16) as u64,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
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
            group("brush", pipelines.brush.get_bind_group_layout(0), &[(0, &params), (1, &cells), (2, &stamps)]);
        let render_group = group("render", pipelines.render.get_bind_group_layout(0), &[(0, &view), (1, &cells)]);
        Self { width, height, cells, params, stamps, view, step_group, brush_group, render_group }
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
        phases: u32,
    ) {
        let stamps = &stamps[stamps.len().saturating_sub(MAX_STAMPS)..];
        let params = [self.width as u32, self.height as u32, stamps.len() as u32, 0];
        queue.write_buffer(&self.params, 0, &words(&params));
        let (gx, gy) = self.groups();
        if !stamps.is_empty() {
            let data: Vec<f32> =
                stamps.iter().flat_map(|s| [s.x, s.y, s.radius, if s.sand { 1.0 } else { 0.0 }]).collect();
            queue.write_buffer(&self.stamps, 0, &floats(&data));
            let mut pass = encoder
                .begin_compute_pass(&wgpu::ComputePassDescriptor { label: Some("brush"), timestamp_writes: None });
            pass.set_pipeline(&pipelines.brush);
            pass.set_bind_group(0, &self.brush_group, &[]);
            pass.dispatch_workgroups(gx, gy, 1);
        }
        // Each phase is its own pass, so the next one starts after every cell has advanced.
        for _ in 0..phases {
            let mut pass = encoder
                .begin_compute_pass(&wgpu::ComputePassDescriptor { label: Some("step"), timestamp_writes: None });
            pass.set_pipeline(&pipelines.step);
            pass.set_bind_group(0, &self.step_group, &[]);
            pass.dispatch_workgroups(gx, gy, 1);
        }
    }

    /// Draws the lattice into `target` with its top-left at `origin` and each cell `cell` pixels.
    pub fn render(
        &self,
        queue: &wgpu::Queue,
        pipelines: &Pipelines,
        encoder: &mut wgpu::CommandEncoder,
        target: &wgpu::TextureView,
        origin: [f32; 2],
        cell: [f32; 2],
    ) {
        let mut data = words(&[self.width as u32, self.height as u32]);
        data.extend(floats(&[origin[0], origin[1], cell[0], cell[1], 0.0, 0.0]));
        queue.write_buffer(&self.view, 0, &data);
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("lattice"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: target,
                resolve_target: None,
                depth_slice: None,
                ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::BLACK), store: wgpu::StoreOp::Store },
            })],
            ..Default::default()
        });
        pass.set_pipeline(&pipelines.render);
        pass.set_bind_group(0, &self.render_group, &[]);
        pass.draw(0..3, 0..1);
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
        for y in 0..h * 2 / 3 {
            for x in 0..w {
                if sim::hash(x, y, 0, 77).is_multiple_of(3) {
                    world.paint(x, y, Material::Sand);
                }
            }
        }
        let lattice = Lattice::new(&device, &pipelines, &world);
        let readback = device.create_buffer(&wgpu::BufferDescriptor {
            label: None,
            size: (w * h * 4) as u64,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });

        let mut phase = 0;
        for chunk in [1, 1, 1, 7, 40, 150] {
            let mut encoder = device.create_command_encoder(&Default::default());
            lattice.simulate(&queue, &pipelines, &mut encoder, &[], chunk);
            encoder.copy_buffer_to_buffer(&lattice.cells, 0, &readback, 0, (w * h * 4) as u64);
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
            let first = gpu.iter().zip(world.words()).position(|(a, b)| a != b);
            assert_eq!(first, None, "after {phase} phases, first mismatch at cell {first:?}");
        }
    }
}
