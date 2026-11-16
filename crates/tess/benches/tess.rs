//! Criterion benchmarks: native tessellation cost by path complexity.

use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};
use tess::tessellate;

/// A regular polygon with straight edges — cheapest possible input.
fn polygon(n: usize) -> String {
    let mut d = String::from("M");
    for i in 0..n {
        let a = (i as f64 / n as f64) * std::f64::consts::TAU;
        let x = 100.0 + 50.0 * a.cos();
        let y = 100.0 + 50.0 * a.sin();
        d.push_str(&format!("{}{},{}", if i == 0 { "" } else { " L" }, x, y));
    }
    d.push_str(" Z");
    d
}

/// A path of cubic curves — the case real icons are made of.
fn curves(n: usize) -> String {
    let mut d = String::from("M10,100");
    for i in 0..n {
        let x = 10.0 + (i as f64) * 8.0;
        d.push_str(&format!(" C{},20 {},180 {},100", x, x + 20.0, x + 40.0));
    }
    d
}

/// Circular arcs, which stress the endpoint-to-centre conversion.
fn arcs(n: usize) -> String {
    let mut d = String::from("M100,100");
    for i in 0..n {
        let x = 100.0 + (i as f64 + 1.0) * 5.0;
        d.push_str(&format!(" A10,10 0 0,1 {},100", x));
    }
    d
}

fn benches(c: &mut Criterion) {
    let mut g = c.benchmark_group("tessellate");

    for &n in &[8usize, 32, 128] {
        let d = polygon(n);
        g.throughput(Throughput::Bytes(d.len() as u64));
        g.bench_with_input(BenchmarkId::new("polygon", n), &d, |b, d| {
            b.iter(|| black_box(tessellate(black_box(d))));
        });
    }

    for &n in &[8usize, 32, 128] {
        let d = curves(n);
        g.throughput(Throughput::Elements(n as u64));
        g.bench_with_input(BenchmarkId::new("cubic_curves", n), &d, |b, d| {
            b.iter(|| black_box(tessellate(black_box(d))));
        });
    }

    for &n in &[8usize, 32] {
        let d = arcs(n);
        g.throughput(Throughput::Elements(n as u64));
        g.bench_with_input(BenchmarkId::new("arcs", n), &d, |b, d| {
            b.iter(|| black_box(tessellate(black_box(d))));
        });
    }

    g.finish();
}

criterion_group!(benches_group, benches);
criterion_main!(benches_group);
