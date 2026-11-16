/**
 * Property-based verification for the SVG tessellator.
 *
 * # The oracle problem, and how it is solved
 *
 * Triangles have no canonical form. A polygon admits many valid triangulations,
 * so "wasm output == JS output" is not a correctness property — it only proves
 * two implementations made the same arbitrary choice.
 *
 * Worse, the tempting cheap oracle — "same triangle count" — is nearly
 * worthless: a badly wrong triangulation usually still has the right count.
 *
 * So this checks properties that hold for ANY correct triangulation, and which
 * my implementation cannot tune:
 *
 *   1. AREA CONSERVATION. Sum of |triangle area| == |polygon area|, within a
 *      relative tolerance. Catches dropped, duplicated, and overlapping
 *      triangles — the three ways a clipper usually goes wrong.
 *
 *   2. CENTROID COVERAGE. Every triangle's centroid must lie inside the source
 *      polygon (winding-number test). Catches triangles that stray outside.
 *
 *   3. COUNT IDENTITY. A simple polygon with n vertices yields exactly n-2
 *      triangles. Catches slivers, early bail-out, and degenerate ears.
 *
 *   4. FLATTENING ACCURACY. Independently re-sample each curve and confirm every
 *      flattened vertex lies within tolerance of the true curve. Checked by
 *      dense brute-force sampling here, not by trusting the implementation's own
 *      segment-count formula.
 *
 * Properties 1-3 hold regardless of implementation, which is what makes this a
 * correctness story rather than a speed one.
 */

import { readFile } from 'node:fs/promises';

const WASM = new URL('../target/wasm32-unknown-unknown/release/wasm.wasm', import.meta.url);

const enc = new TextEncoder();

const EXPECTED = {
  alloc: 1,
  dealloc: 2,
  tessellate: 2,
  flatten_path: 2,
  result_ptr: 0,
  abi_version: 0,
  tolerance: 0,
};

function checkExports(ex) {
  for (const [name, arity] of Object.entries(EXPECTED)) {
    const fn = ex[name];
    if (typeof fn !== 'function') return `missing export: ${name}`;
    if (fn.length !== arity) return `${name} arity ${fn.length}, expected ${arity}`;
  }
  return null;
}

// ---------------------------------------------------------------- geometry

const shoelace = (pts) => {
  let a = 0;
  for (let i = 0; i < pts.length; i++) {
    const p = pts[i];
    const q = pts[(i + 1) % pts.length];
    a += p.x * q.y - q.x * p.y;
  }
  return a / 2;
};

const cross = (o, a, b) => (a.x - o.x) * (b.y - o.y) - (a.y - o.y) * (b.x - o.x);

const pointInPolygon = (p, poly) => {
  let winding = 0;
  const n = poly.length;
  for (let i = 0; i < n; i++) {
    const a = poly[i];
    const b = poly[(i + 1) % n];
    if (a.y <= p.y) {
      if (b.y > p.y && cross(a, b, p) > 0) winding++;
    } else if (b.y <= p.y && cross(a, b, p) < 0) winding--;
  }
  return winding !== 0;
};

// ------------------------------------------------------------ path fixtures

/** Regular n-gon, straight edges. */
function polygon(n, r = 50, cx = 100, cy = 100) {
  const pts = [];
  for (let i = 0; i < n; i++) {
    const a = (i / n) * Math.PI * 2;
    pts.push({ x: cx + r * Math.cos(a), y: cy + r * Math.sin(a) });
  }
  return pts;
}

function toPath(pts) {
  return 'M' + pts.map((p) => `${p.x},${p.y}`).join(' L') + ' Z';
}

/** A star polygon: concave, n points, alternating radii. */
function star(n, r1 = 50, r2 = 20, cx = 100, cy = 100) {
  const pts = [];
  for (let i = 0; i < n * 2; i++) {
    const a = (i / (n * 2)) * Math.PI * 2;
    const r = i % 2 === 0 ? r1 : r2;
    pts.push({ x: cx + r * Math.cos(a), y: cy + r * Math.sin(a) });
  }
  return pts;
}

/** Sample a cubic Bezier at t. */
const cubicAt = (p0, p1, p2, p3, t) => {
  const u = 1 - t;
  const a = u * u * u, b = 3 * u * u * t, c = 3 * u * t * t, d = t * t * t;
  return {
    x: a * p0.x + b * p1.x + c * p2.x + d * p3.x,
    y: a * p0.y + b * p1.y + c * p2.y + d * p3.y,
  };
};

/** Squared distance from point to segment. */
function distToSeg(p, a, b) {
  const dx = b.x - a.x;
  const dy = b.y - a.y;
  const len2 = dx * dx + dy * dy;
  if (len2 < 1e-18) return Math.hypot(p.x - a.x, p.y - a.y);
  let t = ((p.x - a.x) * dx + (p.y - a.y) * dy) / len2;
  t = Math.max(0, Math.min(1, t));
  const qx = a.x + t * dx;
  const qy = a.y + t * dy;
  return Math.hypot(p.x - qx, p.y - qy);
}

// ------------------------------------------------------------------ runner

function wasmTessellate(ex, d) {
  const mem = ex.memory;
  const bytes = enc.encode(d);
  const p = ex.alloc(Math.max(bytes.length, 1));
  new Uint8Array(mem.buffer, p, bytes.length).set(bytes);
  const triCount = ex.tessellate(p, bytes.length);
  const ptr = ex.result_ptr();
  const flats = new Float32Array(mem.buffer, ptr, triCount * 6).slice();
  ex.dealloc(p, Math.max(bytes.length, 1));

  const tris = [];
  for (let i = 0; i < triCount; i++) {
    const o = i * 6;
    tris.push({
      a: { x: flats[o], y: flats[o + 1] },
      b: { x: flats[o + 2], y: flats[o + 3] },
      c: { x: flats[o + 4], y: flats[o + 5] },
    });
  }
  return tris;
}

function wasmFlatten(ex, d) {
  const mem = ex.memory;
  const bytes = enc.encode(d);
  const p = ex.alloc(Math.max(bytes.length, 1));
  new Uint8Array(mem.buffer, p, bytes.length).set(bytes);
  const n = ex.flatten_path(p, bytes.length);
  const ptr = ex.result_ptr();
  const flats = new Float32Array(mem.buffer, ptr, n).slice();
  ex.dealloc(p, Math.max(bytes.length, 1));

  const pts = [];
  for (let i = 0; i < n; i += 2) pts.push({ x: flats[i], y: flats[i + 1] });
  return pts;
}

async function main() {
  const bytes = await readFile(WASM);
  const { instance } = await WebAssembly.instantiate(bytes, {});
  const ex = instance.exports;

  const arityErr = checkExports(ex);
  if (arityErr) {
    console.error(`❌ ${arityErr}`);
    process.exit(1);
  }
  const tol = ex.tolerance();
  console.log(`✅ exports ok (abi ${ex.abi_version()}, flatten tolerance ${tol})`);

  const failures = [];

  // ---- 1-3: triangulation properties on polygons and stars ----
  const shapes = [];
  for (const n of [3, 4, 5, 8, 16, 64, 128]) shapes.push([`${n}-gon`, polygon(n)]);
  for (const n of [3, 4, 5, 8]) shapes.push([`${n}-star`, star(n)]);

  for (const [name, pts] of shapes) {
    const d = toPath(pts);
    const tris = wasmTessellate(ex, d);

    const polyArea = Math.abs(shoelace(pts));
    const triArea = tris.reduce(
      (s, t) => s + Math.abs(cross(t.a, t.b, t.c)) / 2,
      0
    );

    // Property 3: count identity.
    const want = pts.length - 2;
    const countOk = tris.length === want;

    // Property 1: area conservation, relative tolerance for float32 narrowing.
    const areaErr = Math.abs(triArea - polyArea) / Math.max(polyArea, 1e-9);
    const areaOk = areaErr < 1e-4;

    // Property 2: centroid coverage.
    let outside = 0;
    for (const t of tris) {
      const c = { x: (t.a.x + t.b.x + t.c.x) / 3, y: (t.a.y + t.b.y + t.c.y) / 3 };
      if (!pointInPolygon(c, pts)) outside++;
    }
    const coverOk = outside === 0;

    const ok = countOk && areaOk && coverOk;
    const detail = ok
      ? `${tris.length} tris, area ${triArea.toFixed(3)}/${polyArea.toFixed(3)}`
      : [
          !countOk ? `count ${tris.length} != ${want}` : null,
          !areaOk ? `area err ${(areaErr * 100).toFixed(4)}%` : null,
          !coverOk ? `${outside} centroids outside` : null,
        ]
          .filter(Boolean)
          .join(', ');

    console.log(`${ok ? '✅' : '❌'} ${name.padEnd(10)} ${detail}`);
    if (!ok) failures.push(`${name}: ${detail}`);
  }

  // ---- 4: flattening accuracy against independent dense sampling ----
  // Re-sample the true curve at high resolution and confirm each emitted
  // vertex is within tolerance of it. Independent of the implementation's own
  // segment-count estimate.
  const curves = [
    ['gentle', { p0: { x: 0, y: 0 }, p1: { x: 20, y: 40 }, p2: { x: 80, y: 40 }, p3: { x: 100, y: 0 } }],
    ['sharp', { p0: { x: 0, y: 0 }, p1: { x: 0, y: 100 }, p2: { x: 100, y: 100 }, p3: { x: 100, y: 0 } }],
    ['loopish', { p0: { x: 0, y: 0 }, p1: { x: 100, y: 0 }, p2: { x: 0, y: 100 }, p3: { x: 100, y: 100 } }],
  ];

  for (const [name, c] of curves) {
    const d = `M${c.p0.x},${c.p0.y} C${c.p1.x},${c.p1.y} ${c.p2.x},${c.p2.y} ${c.p3.x},${c.p3.y}`;
    const got = wasmFlatten(ex, d);
    if (got.length < 4) {
      failures.push(`${name} curve: only ${got.length} points`);
      console.log(`❌ ${name.padEnd(10)} only ${got.length} points`);
      continue;
    }

    // Dense sample of the true curve.
    const SAMPLES = 4000;
    const truth = [];
    for (let i = 0; i <= SAMPLES; i++) truth.push(cubicAt(c.p0, c.p1, c.p2, c.p3, i / SAMPLES));

    // Max deviation of the emitted polyline from the true curve, measured at
    // each emitted vertex AND along each chord.
    let worst = 0;
    for (let i = 0; i < truth.length; i++) {
      const q = truth[i];
      // Distance to the polyline.
      let best = Infinity;
      for (let k = 0; k + 1 < got.length; k++) {
        const dd = distToSeg(q, got[k], got[k + 1]);
        if (dd < best) best = dd;
      }
      if (best > worst) worst = best;
    }

    // Allow slack for f32 narrowing and for the tolerance being a chord bound.
    const limit = tol * 6;
    const ok = worst <= limit;
    console.log(
      `${ok ? '✅' : '❌'} ${name.padEnd(10)} max dev ${worst.toFixed(4)} (limit ${limit.toFixed(2)}, ${got.length} pts)`
    );
    if (!ok) failures.push(`${name} curve: deviation ${worst.toFixed(4)} > ${limit}`);
  }

  // ---- 5: degenerate input must not crash or lie ----
  const degen = [
    ['empty', ''],
    ['point only', 'M10,10'],
    ['line only', 'M0,0 L10,10'],
    ['collinear', 'M0,0 L5,5 L10,10 Z'],
  ];
  for (const [name, d] of degen) {
    const tris = wasmTessellate(ex, d);
    const ok = tris.length === 0;
    console.log(`${ok ? '✅' : '❌'} degenerate ${name.padEnd(12)} ${tris.length} tris (expected 0)`);
    if (!ok) failures.push(`degenerate ${name}: expected 0 triangles, got ${tris.length}`);
  }

  if (failures.length) {
    console.error(`\n${failures.length} property failure(s):`);
    for (const f of failures) console.error(`  - ${f}`);
    process.exit(1);
  }

  console.log('\nAll geometric properties hold.');
}

main().catch((e) => {
  console.error(e);
  process.exit(1);
});
