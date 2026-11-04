use super::*;
use core::prelude::v1::test;
use windows::Win32::Foundation::HMODULE;

const SIDE: u32 = 64;

struct Offscreen {
    device: ID3D11Device,
    context: ID3D11DeviceContext,
    globals: DirectXGlobalElements,
    target: ID3D11Texture2D,
    target_view: Option<ID3D11RenderTargetView>,
    path_target: ID3D11Texture2D,
    path_target_view: Option<ID3D11RenderTargetView>,
    staging: ID3D11Texture2D,
    texture_view: Option<ID3D11ShaderResourceView>,
    viewport: D3D11_VIEWPORT,
}

impl Offscreen {
    fn new() -> Result<Self> {
        let mut device = None;
        let mut context = None;
        unsafe {
            // A missing device is a test failure, never a successful skip.
            D3D11CreateDevice(
                None,
                D3D_DRIVER_TYPE_WARP,
                HMODULE::default(),
                D3D11_CREATE_DEVICE_BGRA_SUPPORT,
                None,
                D3D11_SDK_VERSION,
                Some(&mut device),
                None,
                Some(&mut context),
            )?;
        }
        let device = device.unwrap();
        let context = context.unwrap();
        let globals = DirectXGlobalElements::new(&device)?;
        let params = GlobalParams {
            gamma_ratios: [1.; 4],
            viewport_size: [SIDE as f32; 2],
            ..Default::default()
        };
        update_buffer(
            &context,
            globals.global_params_buffer.as_ref().unwrap(),
            &[params],
        )?;
        let desc = D3D11_TEXTURE2D_DESC {
            Width: SIDE,
            Height: SIDE,
            MipLevels: 1,
            ArraySize: 1,
            Format: DXGI_FORMAT_R8G8B8A8_UNORM,
            SampleDesc: DXGI_SAMPLE_DESC {
                Count: 1,
                Quality: 0,
            },
            Usage: D3D11_USAGE_DEFAULT,
            BindFlags: D3D11_BIND_RENDER_TARGET.0 as u32,
            ..Default::default()
        };
        let mut target = None;
        let mut target_view = None;
        let mut path_target = None;
        let mut path_target_view = None;
        let mut staging = None;
        let mut texture = None;
        let mut texture_view = None;
        unsafe {
            device.CreateTexture2D(&desc, None, Some(&mut target))?;
            device.CreateRenderTargetView(
                target.as_ref().unwrap(),
                None,
                Some(&mut target_view),
            )?;
            device.CreateTexture2D(
                &D3D11_TEXTURE2D_DESC {
                    SampleDesc: DXGI_SAMPLE_DESC {
                        Count: PATH_MULTISAMPLE_COUNT,
                        Quality: 0,
                    },
                    ..desc
                },
                None,
                Some(&mut path_target),
            )?;
            device.CreateRenderTargetView(
                path_target.as_ref().unwrap(),
                None,
                Some(&mut path_target_view),
            )?;
            device.CreateTexture2D(
                &D3D11_TEXTURE2D_DESC {
                    Usage: D3D11_USAGE_STAGING,
                    BindFlags: 0,
                    CPUAccessFlags: D3D11_CPU_ACCESS_READ.0 as u32,
                    ..desc
                },
                None,
                Some(&mut staging),
            )?;
            let texel = [255u8; 4];
            device.CreateTexture2D(
                &D3D11_TEXTURE2D_DESC {
                    Width: 1,
                    Height: 1,
                    BindFlags: D3D11_BIND_SHADER_RESOURCE.0 as u32,
                    ..desc
                },
                Some(&D3D11_SUBRESOURCE_DATA {
                    pSysMem: texel.as_ptr().cast(),
                    SysMemPitch: 4,
                    ..Default::default()
                }),
                Some(&mut texture),
            )?;
            device.CreateShaderResourceView(
                texture.as_ref().unwrap(),
                None,
                Some(&mut texture_view),
            )?;
            context.VSSetConstantBuffers(0, Some(slice::from_ref(&globals.global_params_buffer)));
            context.PSSetConstantBuffers(0, Some(slice::from_ref(&globals.global_params_buffer)));
        }
        set_rasterizer_state(&device, &context)?;
        let viewport = set_viewport(&context, SIDE as f32, SIDE as f32);
        Ok(Self {
            device,
            context,
            globals,
            target: target.unwrap(),
            target_view,
            path_target: path_target.unwrap(),
            path_target_view,
            staging: staging.unwrap(),
            texture_view,
            viewport,
        })
    }

    fn render<T>(
        &self,
        pipeline: &mut PipelineState<T>,
        data: &[T],
        path: bool,
    ) -> Result<Vec<u8>> {
        let view = if path {
            &self.path_target_view
        } else {
            &self.target_view
        };
        unsafe {
            self.context
                .ClearRenderTargetView(view.as_ref().unwrap(), &[0.; 4]);
            self.context
                .OMSetRenderTargets(Some(slice::from_ref(view)), None);
        }
        pipeline.update_buffer(&self.device, &self.context, data)?;
        let mut cache = DirectXStateCache::default();
        if path {
            pipeline.draw(
                &mut cache,
                &self.context,
                &[self.viewport],
                slice::from_ref(&self.globals.global_params_buffer),
                D3D_PRIMITIVE_TOPOLOGY_TRIANGLELIST,
                data.len() as u32,
                1,
            )?;
        } else {
            // Exercise nonzero primitive range views separately from clip_start.
            pipeline.draw_range_with_texture(
                &mut cache,
                &self.device,
                &self.context,
                slice::from_ref(&self.texture_view),
                &[self.viewport],
                slice::from_ref(&self.globals.global_params_buffer),
                slice::from_ref(&self.globals.sampler),
                1,
                1,
            )?;
        }
        unsafe {
            if path {
                self.context.OMSetRenderTargets(None, None);
                self.context.ResolveSubresource(
                    &self.target,
                    0,
                    &self.path_target,
                    0,
                    DXGI_FORMAT_R8G8B8A8_UNORM,
                );
            }
            self.context.CopyResource(&self.staging, &self.target);
            let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
            self.context
                .Map(&self.staging, 0, D3D11_MAP_READ, 0, Some(&mut mapped))?;
            let mut pixels = Vec::with_capacity((SIDE * SIDE * 4) as usize);
            for y in 0..SIDE as usize {
                pixels.extend_from_slice(slice::from_raw_parts(
                    mapped.pData.cast::<u8>().add(y * mapped.RowPitch as usize),
                    SIDE as usize * 4,
                ));
            }
            self.context.Unmap(&self.staging, 0);
            Ok(pixels)
        }
    }
}

fn rect(x: f32, y: f32, width: f32, height: f32) -> Bounds<ScaledPixels> {
    Bounds::new(
        point(ScaledPixels(x), ScaledPixels(y)),
        size(ScaledPixels(width), ScaledPixels(height)),
    )
}

// Independent circle equations, not the shader's quadrant/SDF implementation.
fn expected_coverage(x: u32, y: u32, rounded: bool) -> f32 {
    let (x, y) = (x as f32 + 0.5, y as f32 + 0.5);
    if x < 8. || x >= 56. || y < 8. || y >= 56. {
        return 0.;
    }
    let mut coverage: f32 = 1.;
    if rounded && x < 28. && y < 28. {
        coverage =
            coverage.min((20.5 - ((x - 28.).powi(2) + (y - 28.).powi(2)).sqrt()).clamp(0., 1.));
    }
    if rounded && x > 40. && y > 40. {
        coverage =
            coverage.min((16.5 - ((x - 40.).powi(2) + (y - 40.).powi(2)).sqrt()).clamp(0., 1.));
    }
    coverage
}

fn check_pixels(label: &str, pixels: &[u8], rounded: bool, subpixel: bool) {
    for y in 0..SIDE {
        for x in 0..SIDE {
            let expected = (expected_coverage(x, y, rounded) * 0.65 * 255.).round() as i32;
            let offset = ((y * SIDE + x) * 4) as usize;
            assert!(
                (pixels[offset] as i32 - expected).abs() <= 2,
                "{label} ({x},{y}) red={} expected={expected}",
                pixels[offset]
            );
            assert_eq!(pixels[offset + 1], 0, "{label} green");
            assert_eq!(pixels[offset + 2], 0, "{label} blue");
            let alpha = if subpixel { 0 } else { expected };
            assert!(
                (pixels[offset + 3] as i32 - alpha).abs() <= 2,
                "{label} ({x},{y}) alpha={} expected={alpha}",
                pixels[offset + 3]
            );
        }
    }
    eprintln!(
        "{label}: checked {} pixels (rounded={rounded})",
        SIDE * SIDE
    );
}

#[test]
fn warp_rounded_descendant_clips_all_primitive_pipelines() -> Result<()> {
    assert_eq!(std::mem::size_of::<GpuContentMask<ScaledPixels>>(), 24);
    assert_eq!(std::mem::size_of::<RoundedClip<ScaledPixels>>(), 32);
    let mut gpu = Offscreen::new()?;
    let bounds = rect(0., 0., 64., 64.);
    let clip_bounds = rect(8., 8., 48., 48.);
    let mut clips = vec![
        RoundedClip {
            bounds: clip_bounds,
            corner_radii: Corners::default()
        };
        42
    ];
    // The ignored prefix catches accidentally dropping clip_start.
    clips[0].bounds = rect(0., 0., 1., 1.);
    clips[1].bounds = rect(0., 0., 1., 1.);
    // Repeated constraints must intersect, not multiply edge opacity.
    for clip in &mut clips[2..] {
        clip.corner_radii.top_left = ScaledPixels(20.);
    }
    clips[41].corner_radii.bottom_right = ScaledPixels(16.);
    gpu.globals
        .upload_rounded_clips(&gpu.device, &gpu.context, &clips[..3])?;
    gpu.globals
        .upload_rounded_clips(&gpu.device, &gpu.context, &clips)?;
    let mut pipelines = DirectXRenderPipelines::new(&gpu.device)?;
    let color = hsla(0., 1., 0.5, 0.65);
    let tile = AtlasTile {
        texture_id: AtlasTextureId {
            index: 0,
            kind: AtlasTextureKind::Monochrome,
        },
        tile_id: TileId(0),
        padding: 0,
        bounds: Bounds::new(
            point(DevicePixels(0), DevicePixels(0)),
            size(DevicePixels(1), DevicePixels(1)),
        ),
    };
    for rounded in [false, true] {
        let mask = GpuContentMask {
            bounds: clip_bounds,
            clip_start: 2,
            clip_count: if rounded { 40 } else { 0 },
        };
        let quad = Quad {
            bounds,
            content_mask: mask,
            background: color.into(),
            ..Default::default()
        };
        check_pixels(
            "quad",
            &gpu.render(&mut pipelines.quad_pipeline, &[quad, quad], false)?,
            rounded,
            false,
        );
        let border = Quad {
            background: Hsla::transparent_black().into(),
            border_color: color,
            border_widths: Edges {
                top: ScaledPixels(64.),
                right: ScaledPixels(64.),
                bottom: ScaledPixels(64.),
                left: ScaledPixels(64.),
            },
            ..quad
        };
        check_pixels(
            "quad border",
            &gpu.render(&mut pipelines.quad_pipeline, &[border, border], false)?,
            rounded,
            false,
        );
        let shadow = Shadow {
            order: 0,
            blur_radius: ScaledPixels(0.),
            bounds,
            corner_radii: Corners::default(),
            content_mask: mask,
            color,
            element_bounds: bounds,
            element_corner_radii: Corners::default(),
            inset: 0,
            pad: 0,
        };
        check_pixels(
            "shadow",
            &gpu.render(&mut pipelines.shadow_pipeline, &[shadow, shadow], false)?,
            rounded,
            false,
        );
        let underline = Underline {
            order: 0,
            pad: 0,
            bounds,
            content_mask: mask,
            color,
            thickness: ScaledPixels(64.),
            wavy: 0,
        };
        check_pixels(
            "underline",
            &gpu.render(
                &mut pipelines.underline_pipeline,
                &[underline, underline],
                false,
            )?,
            rounded,
            false,
        );
        let mono = MonochromeSprite {
            order: 0,
            pad: 0,
            bounds,
            content_mask: mask,
            color,
            tile,
            transformation: TransformationMatrix::unit()
                .translate(point(ScaledPixels(4.), ScaledPixels(2.))),
        };
        check_pixels(
            "mono",
            &gpu.render(&mut pipelines.mono_sprites, &[mono, mono], false)?,
            rounded,
            false,
        );
        let subpixel = SubpixelSprite {
            order: 0,
            pad: 0,
            bounds,
            content_mask: mask,
            color,
            tile,
            transformation: mono.transformation,
        };
        check_pixels(
            "subpixel",
            &gpu.render(
                &mut pipelines.subpixel_sprites,
                &[subpixel, subpixel],
                false,
            )?,
            rounded,
            true,
        );
        // A white texture with opacity produces white, so compare red + alpha separately here.
        let poly = PolychromeSprite {
            order: 0,
            pad: 0,
            grayscale: false,
            opacity: 0.65,
            bounds,
            content_mask: mask,
            corner_radii: Corners::default(),
            tile,
        };
        let mut pixels = gpu.render(&mut pipelines.poly_sprites, &[poly, poly], false)?;
        for pixel in pixels.chunks_exact_mut(4) {
            assert_eq!(pixel[0], pixel[1]);
            assert_eq!(pixel[0], pixel[2]);
            pixel[1] = 0;
            pixel[2] = 0;
        }
        check_pixels("poly", &pixels, rounded, false);
        let vertices: Vec<_> = [
            (0., 0.),
            (64., 0.),
            (0., 64.),
            (0., 64.),
            (64., 0.),
            (64., 64.),
        ]
        .into_iter()
        .map(|(x, y)| PathRasterizationSprite {
            xy_position: point(ScaledPixels(x), ScaledPixels(y)),
            st_position: point(x / 64., 0.),
            color: color.into(),
            bounds: clip_bounds,
            content_mask: mask,
        })
        .collect();
        check_pixels(
            "path intermediate",
            &gpu.render(&mut pipelines.path_rasterization_pipeline, &vertices, true)?,
            rounded,
            false,
        );
    }
    // Round -> rectangle-only -> rounded frames must not retain stale geometry.
    gpu.globals
        .upload_rounded_clips(&gpu.device, &gpu.context, &[])?;
    let mut quad = Quad {
        bounds,
        content_mask: GpuContentMask {
            bounds: clip_bounds,
            clip_start: 0,
            clip_count: 0,
        },
        background: color.into(),
        ..Default::default()
    };
    check_pixels(
        "empty clip frame",
        &gpu.render(&mut pipelines.quad_pipeline, &[quad, quad], false)?,
        false,
        false,
    );
    gpu.globals
        .upload_rounded_clips(&gpu.device, &gpu.context, &clips)?;
    quad.content_mask.clip_start = 2;
    quad.content_mask.clip_count = 40;
    check_pixels(
        "reused clip frame",
        &gpu.render(&mut pipelines.quad_pipeline, &[quad, quad], false)?,
        true,
        false,
    );
    Ok(())
}
