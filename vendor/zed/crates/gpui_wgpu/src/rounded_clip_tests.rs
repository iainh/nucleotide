use super::*;

#[test]
fn rounded_clip_shaders_validate_and_match_cpu_layouts() {
    use wgpu::naga::{TypeInner, front::wgsl, valid};

    let source = format!(
        "enable dual_source_blending;\n{}\n{}",
        include_str!("shaders.wgsl"),
        include_str!("shaders_subpixel.wgsl")
    );
    let module =
        wgsl::parse_str(&source).unwrap_or_else(|e| panic!("{}", e.emit_to_string(&source)));
    valid::Validator::new(valid::ValidationFlags::all(), valid::Capabilities::all())
        .validate(&module)
        .unwrap();

    for (name, size) in [
        ("GpuContentMask", size_of::<GpuContentMask<ScaledPixels>>()),
        ("RoundedClip", size_of::<RoundedClip<ScaledPixels>>()),
        ("Quad", size_of::<Quad>()),
        ("Shadow", size_of::<Shadow>()),
        ("Underline", size_of::<Underline>()),
        ("MonochromeSprite", size_of::<MonochromeSprite>()),
        ("SubpixelSprite", size_of::<SubpixelSprite>()),
        ("PolychromeSprite", size_of::<PolychromeSprite>()),
        (
            "PathRasterizationVertex",
            size_of::<PathRasterizationVertex>(),
        ),
    ] {
        let ty = module
            .types
            .iter()
            .find(|(_, ty)| ty.name.as_deref() == Some(name))
            .unwrap()
            .1;
        let TypeInner::Struct { span, .. } = ty.inner else {
            panic!("{name} is not a struct")
        };
        assert_eq!(span as usize, size, "CPU/WGSL stride mismatch for {name}");
    }
    assert_eq!(size_of::<GpuContentMask<ScaledPixels>>(), 24);
    assert_eq!(size_of::<RoundedClip<ScaledPixels>>(), 32);
}

// Runs the actual production clipping function, rather than a CPU reimplementation.
// A software Vulkan adapter is sufficient, but an absent adapter must not
// silently turn an unexecuted GPU assertion into a successful test.
#[cfg(not(target_family = "wasm"))]
#[test]
fn rounded_clip_gpu_readback_intersects_all_ancestors() {
    use wgpu::util::DeviceExt;

    pollster::block_on(async {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions::default())
            .await
            .expect(
                "rounded clip GPU readback requires a WGPU adapter (software Vulkan is sufficient)",
            );
        eprintln!("Rounded clip readback adapter: {:?}", adapter.get_info());
        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor::default())
            .await
            .unwrap();
        let source = format!(
            "{}\n{}",
            include_str!("shaders.wgsl"),
            r#"
struct ClipQuery {
    position: vec2<f32>,
    mask: GpuContentMask,
}
@group(0) @binding(3) var<storage, read> queries: array<ClipQuery>;
@group(0) @binding(4) var<storage, read_write> results: array<f32>;
@compute @workgroup_size(1)
fn test_clip(@builtin(global_invocation_id) id: vec3<u32>) {
    results[id.x] = rounded_clip_coverage(queries[id.x].position, queries[id.x].mask);
}
"#
        );
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("rounded_clip_readback"),
            source: wgpu::ShaderSource::Wgsl(source.into()),
        });
        let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: None,
            layout: None,
            module: &shader,
            entry_point: Some("test_clip"),
            compilation_options: Default::default(),
            cache: None,
        });
        // Asymmetric radii: TL=8, TR=0, BR=4, BL=0. Repeated clips
        // preserve fractional coverage, and a restrictive 65th ancestor
        // detects any fixed-depth truncation of the shader loop.
        let mut clips = vec![[0.0_f32, 0.0, 20.0, 20.0, 8.0, 0.0, 4.0, 0.0]; 64];
        clips.push([10.0, 10.0, 10.0, 10.0, 0.0, 0.0, 0.0, 0.0]);
        #[repr(C)]
        #[derive(Clone, Copy, Pod, Zeroable)]
        struct Query {
            position: [f32; 2],
            bounds: [f32; 4],
            start: u32,
            count: u32,
        }
        let query = |position, start, count| Query {
            position,
            bounds: [0.0, 0.0, 20.0, 20.0],
            start,
            count,
        };
        let queries = [
            query([0.5, 0.5], u32::MAX, 0), // dummy/rectangular fast path
            query([0.5, 0.5], 0, 1),        // rounded top-left excluded
            query([19.5, 0.5], 0, 1),       // square top-right retained
            query([10.0, 10.0], 0, 64),     // deep intersection retained
            query([5.0, 5.0], 0, 65),       // last ancestor excludes
            query([8.0, 0.25], 0, 1),       // fractional edge
            query([8.0, 0.25], 0, 64),      // duplicate edge must not darken
            query([5.0, 5.0], 64, 1),       // nonzero range offset
        ];
        let clips_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: None,
            contents: bytemuck::cast_slice(&clips),
            usage: wgpu::BufferUsages::STORAGE,
        });
        let queries_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: None,
            contents: bytemuck::cast_slice(&queries),
            usage: wgpu::BufferUsages::STORAGE,
        });
        let size = (queries.len() * size_of::<f32>()) as u64;
        let output = device.create_buffer(&wgpu::BufferDescriptor {
            label: None,
            size,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });
        let readback = device.create_buffer(&wgpu::BufferDescriptor {
            label: None,
            size,
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let bindings = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: None,
            layout: &pipeline.get_bind_group_layout(0),
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: clips_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: queries_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 4,
                    resource: output.as_entire_binding(),
                },
            ],
        });
        let mut encoder = device.create_command_encoder(&Default::default());
        {
            let mut pass = encoder.begin_compute_pass(&Default::default());
            pass.set_pipeline(&pipeline);
            pass.set_bind_group(0, &bindings, &[]);
            pass.dispatch_workgroups(queries.len() as u32, 1, 1);
        }
        encoder.copy_buffer_to_buffer(&output, 0, &readback, 0, size);
        queue.submit([encoder.finish()]);
        let (tx, rx) = std::sync::mpsc::channel();
        readback
            .slice(..)
            .map_async(wgpu::MapMode::Read, move |result| tx.send(result).unwrap());
        device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
        rx.recv().unwrap().unwrap();
        let mapped = readback.slice(..).get_mapped_range();
        let actual: &[f32] = bytemuck::cast_slice(&mapped);
        let expected = [1.0, 0.0, 1.0, 1.0, 0.0, 0.75, 0.75, 0.0];
        for (i, (&actual, expected)) in actual.iter().zip(expected).enumerate() {
            assert!(
                (actual - expected).abs() < 0.001,
                "query {i}: expected {expected}, got {actual}"
            );
        }
    });
}
