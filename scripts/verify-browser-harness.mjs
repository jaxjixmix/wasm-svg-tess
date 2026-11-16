/**
 * Headless coverage for the browser harness (`web/js/tess.js`).
 *
 * `tess.js` exists as a module rather than inline page script specifically so
 * this file can run it. That is the fix for the failure mode found in
 * wasm-image-lab: a benchmark living inside index.html cannot be tested, so a
 * broken call there stays invisible while CI passes.
 *
 * Runs the real `runHarness`, then re-verifies the same geometric properties
 * independently, so a silent regression in the harness itself is caught.
 */

import { readFile } from 'node:fs/promises';
import { runHarness, checkExports, tessellate, toPath, polygonPts, shoelace } from '../web/js/tess.js';

const WASM = new URL('../target/wasm32-unknown-unknown/release/wasm.wasm', import.meta.url);

async function main() {
  const bytes = await readFile(WASM);
  const { instance } = await WebAssembly.instantiate(bytes, {});
  const ex = instance.exports;

  const arityErr = checkExports(ex);
  if (arityErr) {
    console.error(`❌ ${arityErr}`);
    process.exit(1);
  }
  console.log('✅ wasm exports present with expected arity');

  const { properties, timings } = runHarness(ex, {});

  const bad = properties.filter((p) => !p.ok);
  if (bad.length) {
    console.error(`❌ ${bad.length}/${properties.length} property failures:`);
    for (const b of bad) console.error(`   ${b.name}: ${b.detail}`);
    process.exit(1);
  }
  console.log(`✅ harness: ${properties.length}/${properties.length} shapes satisfy properties`);

  if (!timings.length) {
    console.error('❌ no timing rows produced');
    process.exit(1);
  }
  for (const t of timings) {
    if (!(t.ms > 0) || !Number.isFinite(t.ms)) {
      console.error(`❌ ${t.label}: invalid timing ${t.ms}`);
      process.exit(1);
    }
  }
  console.log(`✅ harness: ${timings.length} timing rows finite and positive`);

  // Independent re-check of one shape, outside the harness, so a bug in
  // checkTriangulation() cannot mask a real geometric failure.
  const pts = polygonPts(16);
  const tris = tessellate(ex, toPath(pts));
  const polyArea = Math.abs(shoelace(pts));
  let triArea = 0;
  for (const t of tris) {
    triArea += Math.abs(
      ((t.b.x - t.a.x) * (t.c.y - t.a.y) - (t.c.x - t.a.x) * (t.b.y - t.a.y)) / 2
    );
  }
  const relErr = Math.abs(triArea - polyArea) / polyArea;
  if (relErr > 1e-4) {
    console.error(`❌ independent area re-check failed: ${(relErr * 100).toFixed(4)}%`);
    process.exit(1);
  }
  console.log(`✅ independent area re-check: ${(relErr * 100).toFixed(5)}% error`);

  console.log('\nSVG harness exercised end-to-end.');
}

main().catch((e) => {
  console.error(e);
  process.exit(1);
});
