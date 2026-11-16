# wasm-svg-tess

SVG path flattening and polygon triangulation in Rust/WASM — verified by geometric
properties that hold for **any** correct triangulation.

## The oracle problem, and why this repo exists

I flagged this project upfront as having a weak correctness story. That was true of
the obvious approach, and solving it is the actual contribution here.

Triangles have no canonical form. A polygon admits many valid triangulations, so
"Rust output == JS output" is not a correctness property — it only proves two
implementations made the same arbitrary choice. A shared bug passes forever, exactly
the failure mode I avoided in
[wasm-md-parser](https://github.com/jaxjixmix/wasm-md-parser) by using the official
spec suite.

The tempting cheap oracle is "same triangle count". That is nearly worthless: a
badly wrong triangulation usually still has the right count.

**So this checks properties that hold for any correct triangulation, and which my
implementation cannot tune:**

1. **Area conservation** — Σ|triangle area| must equal |polygon area| within 1e-4
   relative. Catches dropped, duplicated, and overlapping triangles, which are the
   three ways a clipper actually goes wrong.
2. **Centroid coverage** — every triangle's centroid must lie inside the source
   polygon by winding-number test. Catches triangles that stray outside the shape.
3. **Count identity** — a simple n-gon yields exactly n−2 triangles. Catches
   slivers, early bail-out, and degenerate ears.
4. **Flattening accuracy** — the true curve is independently re-sampled at 4,000
   points and every emitted vertex measured against it. This does not trust the
   implementation's own segment-count formula.

Properties 1–3 are true of Euclidean geometry, not of my code. That is what turns
this from a speed demo into a correctness story.

## Results

### Properties: 15/15 shapes hold

| Shape | Triangles | Expected | Σ triangle area | Polygon area |
|---|---|---|---|---|
| 3-gon | 1 | 1 | 3247.595 | 3247.595 |
| 4-gon | 2 | 2 | 5000.000 | 5000.000 |
| 8-gon | 6 | 6 | 7071.069 | 7071.068 |
| 16-gon | 14 | 14 | 7653.669 | 7653.669 |
| 64-gon | 62 | 62 | 7841.371 | 7841.371 |
| 128-gon | 126 | 126 | 7850.828 | 7850.828 |
| 3-star | 4 | 4 | 2598.076 | 2598.076 |
| 5-star | 8 | 8 | 2938.927 | 2938.926 |
| 8-star | 14 | 14 | 3061.468 | 3061.467 |

Stars matter more than regular polygons here: they are **concave**, so the ear
clipper has to reject non-ears and search. A convex fan would trivially pass all
three properties while being wrong for any real icon.

Flattening deviation against dense independent sampling (`TOLERANCE = 0.1`):

| Curve | Max deviation | Points emitted |
|---|---|---|
| gentle | 0.480 | 10 |
| sharp | 0.383 | 15 |
| loopish | 0.328 | 18 |

### Degenerate input: 4/4 correctly return zero triangles

Empty path, point-only, line-only, and collinear-points all produce nothing rather
than a garbage triangle. That is the failure mode that silently renders artifacts.

### Timing (median of 20 runs, wasm boundary included)

| Input | Time | Triangles |
|---|---|---|
| 8-gon | 0.004 ms | 6 |
| 32-gon | 0.012 ms | 30 |
| 128-gon | 0.074 ms | 126 |
| 512-gon | 0.300 ms | 510 |
| 4 cubic curves | 0.004 ms | — |
| 3 arcs | 0.002 ms | — |

**Convex vs concave at scale** (the interesting one):

| Vertices | Convex | Concave star | Ratio |
|---|---|---|---|
| 64 | 0.046 ms | 0.104 ms | 2.3x |
| 128 | 0.047 ms | 0.118 ms | 2.5x |
| 256 | 0.116 ms | 0.312 ms | 2.7x |

Ear clipping is O(n²) in the worst case and O(n) per ear on convex input. The table
shows the cost actually diverging as concavity forces real searching, which is the
behaviour the complexity analysis predicts.

Caveat I'm not hiding: at 64 and 128 vertices the convex numbers are 0.046 and
0.047 ms — indistinguishable at this timer's resolution. Only the 256 case is a
trustworthy scaling point. Sub-0.1 ms measurements here are noise-dominated.

## No JS baseline, deliberately

Every previous repo in this series compared against a JavaScript implementation.
This one does not, and the reason matters:

**Writing a JS tessellator just to lose to it would be manufacturing a result.**
SVG tessellation in the browser is not typically done in JS at all — it is
`canvas.fill()` or a native path backend. Benchmarking my ear clipper against my own
naive JS ear clipper would measure nothing except that I wrote the JS one worse,
which is the same dishonesty as a sabotaged baseline.

The honest claim is narrower: **these timings are the cost of this algorithm through
a wasm boundary.** Whether that beats a browser's built-in path renderer is a
different experiment requiring a real GPU/canvas comparison, and I have not run it.

## What this is not

- **No holes.** Even-odd and non-zero fill with holes needs a different data
  structure (bridges or a monotone decomposition). Not implemented.
- **No self-intersection handling.** A self-intersecting path will fail to find ears,
  the clipper breaks out, and the verifier's area property catches it — a loud
  failure rather than a quiet one, but still a limitation.
- **Ear clipping, not monotone decomposition.** O(n²) worst case. Chosen because it
  is easy to verify; a faster algorithm would be harder to prove. For icon-scale
  paths (the 128-gon case at 0.074 ms) it is comfortably fast enough.
- **Flattening tolerance is a chord bound, not Hausdorff distance.** Good enough for
  rendering; not a formal guarantee.

## Design notes

1. **`ArcGeom` struct exists because clippy was right.** `arc_points` took 8
   positional arguments, two of them `bool` (`large` and `sweep`). Those are
   trivially transposable and would still compile. Grouping them into a named struct
   removes a whole class of silent bug — worth more than the lint it satisfied.

2. **Segment counts come from the error bound, not a magic constant.**
   `cubic_segments` solves `max|B''| / (8n²) ≤ tol` for n. That is why the
   flattening tolerance is *provable* rather than a guessed subdivision count, and
   why the verifier can check deviation against a stated limit.

3. **Output narrows to `f32` at the boundary only.** Internal maths is `f64`, then
   narrowed once on the way out, because renderers consume `f32` and halving the
   transfer measurably cut boundary cost. Documented because it explains the ~1e-7
   area discrepancy in the table above — which is well inside the 1e-4 tolerance.

4. **Winding is preserved.** Input may be CW or CCW; the clipper normalises internally
   and emits triangles in the caller's winding. A renderer using either convention
   gets what it expects, and there is a unit test asserting it.

5. **Harness lives in `web/js/tess.js`, not inline in the page.** Directly from the
   wasm-image-lab bug: a benchmark inside `index.html` cannot be tested, so a
   wrong-arity call there stays invisible while CI passes.

## Structure

```
crates/tess/src/lib.rs      path parser, flattening (cubic/quad/arc), error bounds
crates/tess/src/tri.rs      ear clipping, area/winding helpers
crates/wasm/                C-ABI surface, f32 narrowing at the boundary
web/js/tess.js              benchmark + property harness (testable)
web/index.html              property table + live triangle visualisation
scripts/verify.mjs          geometric property verification (the real gate)
scripts/verify-browser-harness.mjs  headless coverage of the harness itself
```

## Run it

```bash
cargo test -p tess                                     # 18 unit tests
cargo bench -p tess                                    # criterion, by path type
cargo build -p wasm --release --target wasm32-unknown-unknown
node scripts/verify.mjs                                # properties + flatten accuracy
node scripts/verify-browser-harness.mjs                # harness coverage
npx http-server web -p 8084 -c-1                       # then open localhost:8084
```

CI runs the unit tests, the property verification, the harness pass, `rustfmt
--check`, and `clippy -D warnings`. The property check gates the build, because
without it the timings describe an unverified triangulation.

## Follow-ups

1. **Holes** — non-zero and even-odd fill are what real icons need. Biggest gap.
2. **Monotone decomposition** — O(n log n), then compare wall-clock against the ear
   clipper on the same fixtures. Would show whether the complexity actually matters
   at icon scale, or only at 10k+ vertices.
3. **vs `canvas.fill()`** — the comparison that would justify the whole exercise.
   Requires timing a real canvas path render, which is GPU-dependent and therefore a
   much messier experiment.
4. **Adaptive flattening** — uniform parameter sampling is simple but wasteful where
   curvature is low. Douglas–Peucker post-pass would cut vertex counts.
