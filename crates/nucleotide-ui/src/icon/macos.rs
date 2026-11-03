//! Main-thread SF Symbols rasterization. Only owned BGRA pixels enter the cache.

use std::{cell::RefCell, collections::VecDeque, sync::Arc};

use gpui::{Bounds, Hsla, Pixels, RenderImage, Window};
use image::{Frame, RgbaImage};
use objc2::{AnyThread, ClassType, MainThreadMarker, exception, rc::autoreleasepool, sel};
use objc2_app_kit::{
    NSBitmapFormat, NSBitmapImageRep, NSCompositingOperation, NSDeviceRGBColorSpace,
    NSGraphicsContext, NSImage,
};
use objc2_foundation::{NSPoint, NSRect, NSSize, NSString};

const MAX_ENTRIES: usize = 128;
const MAX_CACHE_BYTES: usize = 8 * 1024 * 1024;
const MAX_DIMENSION: u32 = 1024;

#[derive(Clone, Copy, PartialEq, Eq)]
struct CacheKey {
    symbol: &'static str,
    width: u32,
    height: u32,
    // Preserve the supplied tint, not just its rounded 8-bit representation.
    tint: [u32; 4],
}

type CacheEntry = (CacheKey, Option<Arc<RenderImage>>);

thread_local! {
    // Negative lookups are bounded too. No Objective-C object survives a paint.
    static CACHE: RefCell<VecDeque<CacheEntry>> = const { RefCell::new(VecDeque::new()) };
}

/// Paint a runtime-provided SF Symbol, or let the caller use its SVG fallback.
pub(super) fn paint(
    symbol: &'static str,
    bounds: Bounds<Pixels>,
    color: Hsla,
    window: &mut Window,
) -> bool {
    if MainThreadMarker::new().is_none() {
        return false;
    }

    let scale = window.scale_factor();
    let Some(width) = physical_dimension(f32::from(bounds.size.width), scale) else {
        return false;
    };
    let Some(height) = physical_dimension(f32::from(bounds.size.height), scale) else {
        return false;
    };
    let rgba = color.to_rgb();
    let tint = [rgba.r, rgba.g, rgba.b, rgba.a];
    if tint.iter().any(|component| !component.is_finite()) {
        return false;
    }
    let key = CacheKey {
        symbol,
        width,
        height,
        tint: tint.map(f32::to_bits),
    };

    let cached = CACHE.with(|cache| {
        let mut cache = cache.borrow_mut();
        let index = cache.iter().position(|(candidate, _)| *candidate == key)?;
        let entry = cache.remove(index)?;
        let image = entry.1.clone();
        cache.push_back(entry);
        Some(image)
    });
    let image = match cached {
        Some(image) => image,
        None => {
            // AppKit can throw for rendering failures as well as return nil.
            // Both cases must leave the bundled fallback available.
            let image = autoreleasepool(|_| {
                exception::catch(|| rasterize(key, tint))
                    .ok()
                    .flatten()
                    .map(Arc::new)
            });
            CACHE.with(|cache| {
                let mut cache = cache.borrow_mut();
                cache.push_back((key, image.clone()));
                let mut bytes: usize = cache
                    .iter()
                    .filter(|(_, image)| image.is_some())
                    .map(|(key, _)| key.width as usize * key.height as usize * 4)
                    .sum();
                while cache.len() > MAX_ENTRIES || bytes > MAX_CACHE_BYTES {
                    if let Some((key, Some(_))) = cache.pop_front() {
                        bytes -= key.width as usize * key.height as usize * 4;
                    }
                }
            });
            image
        }
    };

    let Some(image) = image else {
        return false;
    };
    window
        .paint_image(bounds, Default::default(), image, 0, false)
        .is_ok()
}

fn physical_dimension(logical: f32, scale: f32) -> Option<u32> {
    if !logical.is_finite() || !scale.is_finite() || logical <= 0.0 || scale <= 0.0 {
        return None;
    }
    let physical = (logical * scale).ceil();
    (physical > 0.0 && physical <= MAX_DIMENSION as f32).then_some(physical as u32)
}

/// Restore the canvas's AppKit context even if native drawing unwinds.
struct GraphicsState;

impl Drop for GraphicsState {
    fn drop(&mut self) {
        NSGraphicsContext::restoreGraphicsState_class();
    }
}

fn rasterize(key: CacheKey, tint: [f32; 4]) -> Option<RenderImage> {
    // This class method was introduced in macOS 11; avoid sending an unknown
    // selector on older systems. Missing names on newer systems return nil.
    if !NSImage::class()
        .metaclass()
        .responds_to(sel!(imageWithSystemSymbolName:accessibilityDescription:))
    {
        return None;
    }
    let image = NSImage::imageWithSystemSymbolName_accessibilityDescription(
        &NSString::from_str(key.symbol),
        None,
    )?;
    let intrinsic = image.size();
    if !intrinsic.width.is_finite()
        || !intrinsic.height.is_finite()
        || intrinsic.width <= 0.0
        || intrinsic.height <= 0.0
    {
        return None;
    }

    let row_bytes = key.width as usize * 4;
    // SAFETY: null planes asks AppKit to allocate its own storage. All sizes
    // are positive, bounded, and describe interleaved 8-bit RGBA (alpha last).
    let bitmap = unsafe {
        NSBitmapImageRep::initWithBitmapDataPlanes_pixelsWide_pixelsHigh_bitsPerSample_samplesPerPixel_hasAlpha_isPlanar_colorSpaceName_bitmapFormat_bytesPerRow_bitsPerPixel(
            NSBitmapImageRep::alloc(),
            std::ptr::null_mut(),
            key.width as isize,
            key.height as isize,
            8,
            4,
            true,
            false,
            NSDeviceRGBColorSpace,
            NSBitmapFormat::empty(),
            row_bytes as isize,
            32,
        )?
    };
    if bitmap.bytesPerRow() != row_bytes as isize {
        return None;
    }
    let data = bitmap.bitmapData();
    if data.is_null() {
        return None;
    }
    let byte_count = row_bytes * key.height as usize;
    // SAFETY: the live bitmap owns at least bytesPerRow * pixelsHigh bytes.
    // No other code has access to this newly allocated bitmap.
    unsafe { std::ptr::write_bytes(data, 0, byte_count) };

    let context = NSGraphicsContext::graphicsContextWithBitmapImageRep(&bitmap)?;
    {
        NSGraphicsContext::saveGraphicsState_class();
        let _restore = GraphicsState;
        NSGraphicsContext::setCurrentContext(Some(&context));
        let width = f64::from(key.width);
        let height = f64::from(key.height);
        let fit = (width / intrinsic.width).min(height / intrinsic.height);
        let size = NSSize::new(intrinsic.width * fit, intrinsic.height * fit);
        let destination = NSRect::new(
            NSPoint::new((width - size.width) / 2.0, (height - size.height) / 2.0),
            size,
        );
        image.drawInRect_fromRect_operation_fraction(
            destination,
            NSRect::new(NSPoint::new(0.0, 0.0), intrinsic),
            NSCompositingOperation::SourceOver,
            1.0,
        );
    }

    // Use only native coverage, never AppKit's template colour or colour-space
    // conversion. This gives the exact requested tint (to 8-bit precision),
    // including its alpha, in GPUI's straight-alpha BGRA format.
    let mut pixels = RgbaImage::new(key.width, key.height);
    let [red, green, blue, alpha] = tint.map(|value| value.clamp(0.0, 1.0));
    let channel = |value: f32| (value * 255.0).round() as u8;
    let mut has_coverage = false;
    // SAFETY: the bitmap is still alive and drawing has finished. The slice
    // does not escape this function; only copied, Rust-owned pixels do.
    let source = unsafe { std::slice::from_raw_parts(data, byte_count) };
    for (source, destination) in source.chunks_exact(4).zip(pixels.pixels_mut()) {
        has_coverage |= source[3] != 0;
        destination.0 = [
            channel(blue),
            channel(green),
            channel(red),
            (f32::from(source[3]) * alpha).round() as u8,
        ];
    }
    has_coverage.then(|| RenderImage::new([Frame::new(pixels)]))
}
