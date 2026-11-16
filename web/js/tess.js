/**
 * Benchmark + property harness for the tessellator.
 *
 * Extracted into a module (not inline in index.html) so CI can run the real
 * code path headlessly — the structural lesson from wasm-image-lab, where a
 * wrong-arity call hid in the page because no test could reach it.
 *
 * Returns plain data. DOM rendering belongs in index.html.
 */

export const EXPECTED_EXPORTS = {
  alloc: 1,
  dealloc: 2,
  tessellate: 2,
  flatten_path: 2,
  result_ptr: 0,
  abi_version: 0,
  tolerance: 0,
};

/** Guard the ABI: name + arity, so caller/callee drift fails loudly. */
export function checkExports(ex) {
  for (const [name, arity] of Object.entries(EXPECTED_EXPORTS)) {
    const fn = ex[name];
    if (typeof fn !== 'function') return `missing export: ${name}`;
    if (fn.length !== arity) return `${name} arity ${fn.length}, expected ${arity}`;
  }
  return null;
}

export const median = (xs) => {
  const a = [...xs].sort((p, q) => p - q);
  const m = a.length >> 1;
  return a.length % 2 ? a[m] : (a[m - 1] + a[m]) / 2;
};

export function timeIt(fn, warmup = 3, runs = 15) {
  for (let i = 0; i < warmup; i++) fn();
  const s = [];
  for (let i = 0; i < runs; i++) {
    const t0 = performance.now();
    fn();
    s.push(performance.now() - t0);
  }
  return median(s);
}

// ---------------------------------------------------------------- geometry

const enc = new TextEncoder();

export const shoelace = (pts) => {
  let a = 0;
  for (let i = 0; i < pts.length; i++) {
    const p = pts[i];
    const q = pts[(i + 1) % pts.length];
    a += p.x * q.y - q.x * p.y;
  }
  return a / 2;
};

const cross = (o, a, b) => (a.x - o.x) * (b.y - o.y) - (a.y - o.y) * (b.x - o.x);

export const pointInPolygon = (p, poly) => {
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

// --------------------------------------------------------------- fixtures

export function polygonPts(n, r = 50, cx = 100, cy = 100) {
  const pts = [];
  for (let i = 0; i < n; i++) {
    const a = (i / n) * Math.PI * 2;
    pts.push({ x: cx + r * Math.cos(a), y: cy + r * Math.sin(a) });
  }
  return pts;
}

export function starPts(n, r1 = 50, r2 = 20, cx = 100, cy = 100) {
  const pts = [];
  for (let i = 0; i < n * 2; i++) {
    const a = (i / (n * 2)) * Math.PI * 2;
    const r = i % 2 === 0 ? r1 : r2;
    pts.push({ x: cx + r * Math.cos(a), y: cy + r * Math.sin(a) });
  }
  return pts;
}

export const toPath = (pts) => 'M' + pts.map((p) => `${p.x},${p.y}`).join(' L') + ' Z';

/** Path fixtures used by both the page and CI. */
export function fixtures() {
  const out = [];
  for (const n of [3, 4, 5, 8, 16, 64, 128]) out.push([`${n}-gon`, polygonPts(n), polygonPts(n)]);
  for (const n of [3, 4, 5, 8]) out.push([`${n}-star`, starPts(n), starPts(n)]);
  return out;
}

// --------------------------------------------------------------- runners

export function tessellate(ex, d) {
  const mem = ex.memory;
  const bytes = enc.encode(d);
  const p = ex.alloc(Math.max(bytes.length, 1));
  new Uint8Array(mem.buffer, p, bytes.length).set(bytes);
  const n = ex.tessellate(p, bytes.length);
  const ptr = ex.result_ptr();
  // Snapshot immediately: the next call overwrites the buffer.
  const flats = new Float32Array(mem.buffer, ptr, n * 6).slice();
  ex.dealloc(p, Math.max(bytes.length, 1));

  const tris = [];
  for (let i = 0; i < n; i++) {
    const o = i * 6;
    tris.push({
      a: { x: flats[o], y: flats[o + 1] },
      b: { x: flats[o + 2], y: flats[o + 3] },
      c: { x: flats[o + 4], y: flats[o + 5] },
    });
  }
  return tris;
}

/**
 * Verify the three implementation-independent triangulation properties.
 * Returns `{ ok, count, triArea, polyArea, outside, detail }`.
 */
export function checkTriangulation(tris, pts) {
  const polyArea = Math.abs(shoelace(pts));
  const triArea = tris.reduce((s, t) => s + Math.abs(cross(t.a, t.b, t.c)) / 2, 0);
  const want = pts.length - 2;
  const countOk = tris.length === want;
  const areaErr = Math.abs(triArea - polyArea) / Math.max(polyArea, 1e-9);
  const areaOk = areaErr < 1e-4;

  let outside = 0;
  for (const t of tris) {
    const c = { x: (t.a.x + t.b.x + t.c.x) / 3, y: (t.a.y + t.b.y + t.c.y) / 3 };
    if (!pointInPolygon(c, pts)) outside++;
  }
  const coverOk = outside === 0;

  return {
    ok: countOk && areaOk && coverOk,
    count: tris.length,
    want,
    triArea,
    polyArea,
    outside,
    areaErr,
    countOk,
    areaOk,
    coverOk,
    detail: [
      !countOk ? `count ${tris.length} != ${want}` : null,
      !areaOk ? `area err ${(areaErr * 100).toFixed(4)}%` : null,
      !coverOk ? `${outside} centroids outside` : null,
    ]
      .filter(Boolean)
      .join(', '),
  };
}

/**
 * Full harness: property checks + timings.
 *
 * Timings are native-Rust-only (no JS baseline) by design — there is no
 * reference JS tessellator here, and inventing one would produce a number that
 * says nothing. See README.
 */
export function runHarness(ex, { log = () => {} } = {}) {
  const properties = [];

  for (const [name, pts, pathPts] of fixtures()) {
    const tris = tessellate(ex, toPath(pathPts));
    const r = checkTriangulation(tris, pts);
    properties.push({ name, ...r });
    if (!r.ok) log(`FAIL ${name}: ${r.detail}`);
  }

  // Timing by shape: straight edges vs curves vs arcs.
  const timings = [];

  for (const [label, d, elements] of [
    ['8-gon', toPath(polygonPts(8)), 8],
    ['128-gon', toPath(polygonPts(128)), 128],
    ['8 curves', 'M10,100 C30,20 50,20 70,100 C90,20 110,20 130,100 C150,20 170,20 190,100 C210,20 230,20 250,100', 4],
    ['8 arcs', 'M100,100 A10,10 0 0,1 110,100 A10,10 0 0,1 120,100 A10,10 0 0,1 130,100 A10,10 0 0,1 140,100', 4],
  ]) {
    const ms = timeIt(() => tessellate(ex, d));
    timings.push({ label, ms, elements, perElement: ms / elements });
    log(`${label.padEnd(10)} ${ms.toFixed(3)} ms`);
  }

  return { properties, timings };
}
