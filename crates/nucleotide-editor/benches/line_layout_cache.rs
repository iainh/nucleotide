use criterion::{BatchSize, BenchmarkId, Criterion, criterion_group, criterion_main};
use gpui::{Pixels, point, px};
use nucleotide_editor::{LineLayout, LineLayoutCache};
use rand::{Rng, SeedableRng, rngs::StdRng};

fn build_cache(line_count: usize, line_height: Pixels) -> LineLayoutCache {
    let cache = LineLayoutCache::new();
    let line_height_value: f32 = line_height.into();
    for line_idx in 0..line_count {
        let layout = LineLayout {
            line_idx,
            shaped_line: Default::default(),
            origin: point(px(0.0), px(line_idx as f32 * line_height_value)),
            segment_char_offset: 0,
            text_start_byte_offset: 0,
        };
        cache.push(layout);
    }
    cache
}

fn bench_find_line_by_index(c: &mut Criterion) {
    let mut group = c.benchmark_group("find_line_by_index");
    let line_height = px(18.0);

    for &line_count in &[128usize, 1024, 8192] {
        group.bench_function(BenchmarkId::from_parameter(line_count), |b| {
            b.iter_batched(
                || {
                    let cache = build_cache(line_count, line_height);
                    (cache, line_count)
                },
                |(cache, line_count)| {
                    let mut rng = StdRng::seed_from_u64(42);
                    for _ in 0..line_count {
                        let line_idx = rng.gen_range(0..line_count);
                        criterion::black_box(cache.find_line_by_index(line_idx));
                    }
                },
                BatchSize::SmallInput,
            );
        });
    }
    group.finish();
}

fn bench_find_line_at_position(c: &mut Criterion) {
    let mut group = c.benchmark_group("find_line_at_position");
    let line_height = px(18.0);
    let bounds_width = px(800.0);

    for &line_count in &[128usize, 1024, 8192] {
        group.bench_function(BenchmarkId::from_parameter(line_count), |b| {
            b.iter_batched(
                || {
                    let cache = build_cache(line_count, line_height);
                    (cache, line_count)
                },
                |(cache, line_count)| {
                    let mut rng = StdRng::seed_from_u64(1337);
                    let line_height_value: f32 = line_height.into();
                    for _ in 0..line_count {
                        let y = rng.gen_range(0..line_count) as f32 * line_height_value;
                        let position = point(px(10.0), px(y));
                        criterion::black_box(cache.find_line_at_position(
                            position,
                            bounds_width,
                            line_height,
                        ));
                    }
                },
                BatchSize::SmallInput,
            );
        });
    }
    group.finish();
}

criterion_group!(
    benches,
    bench_find_line_by_index,
    bench_find_line_at_position
);
criterion_main!(benches);
