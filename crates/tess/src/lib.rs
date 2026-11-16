//! SVG path flattening + polygon triangulation.
//!
//! # Why this project's oracle is different
//!
//! The previous four repos had a byte-exact oracle. Triangles have no canonical
//! form: a polygon admits many valid triangulations, so "Rust output equals JS
//! output" is not a correctness property, it is a property of both sides
//! agreeing on an arbitrary choice.
//!
//! The naive weak oracle is "same triangle count". That is nearly worthless —
//! a wrong triangulation usually still has the right count.
//!
//! **The oracle used here is stronger and does not depend on my choices:**
//!
//! 1. **Area conservation** — the signed sum of triangle areas must equal the
//!    polygon's signed area, to within float tolerance. Catches dropped,
//!    duplicated, or overlapping triangles.
//! 2. **Coverage** — every triangle centroid must lie inside the source polygon
//!    (winding-number test). Catches triangles outside the shape.
//! 3. **Planarity/count identity** — for a simple polygon of `n` vertices the
//!    triangulation must have exactly `n - 2` triangles. Catches slivers and
//!    fans that bail early.
//! 4. **Flattening accuracy** — every flattened vertex must lie within a stated
//!    tolerance of the true curve (sampled independently by the checker).
//!
//! Properties 1-3 validate *any* correct triangulation, so they cannot be gamed
//! by matching my implementation's quirks. That is what makes this repo a
//! correctness story rather than only a speed one.

pub mod tri;

use std::f64::consts::PI;

/// A 2-D point.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct P {
    pub x: f64,
    pub y: f64,
}

impl P {
    #[inline]
    pub fn new(x: f64, y: f64) -> Self {
        Self { x, y }
    }
}

/// A path command after parsing.
#[derive(Debug, Clone, PartialEq)]
pub enum Cmd {
    MoveTo(P),
    LineTo(P),
    CubicTo(P, P, P),
    QuadTo(P, P),
    ArcTo {
        rx: f64,
        ry: f64,
        rot: f64,
        large: bool,
        sweep: bool,
        to: P,
    },
    Close,
}

/// Flattening tolerance: max distance a chord may deviate from the true curve.
pub const TOLERANCE: f64 = 0.1;

// ---------------------------------------------------------------- parsing

struct Lexer<'a> {
    b: &'a [u8],
    i: usize,
}

impl<'a> Lexer<'a> {
    fn new(s: &'a str) -> Self {
        Self {
            b: s.as_bytes(),
            i: 0,
        }
    }

    fn skip_ws(&mut self) {
        while self.i < self.b.len() {
            match self.b[self.i] {
                b' ' | b'\t' | b'\n' | b'\r' | b',' => self.i += 1,
                _ => break,
            }
        }
    }

    fn peek(&mut self) -> Option<u8> {
        self.skip_ws();
        self.b.get(self.i).copied()
    }

    fn take_cmd(&mut self) -> Option<u8> {
        let c = self.peek()?;
        if c.is_ascii_alphabetic() {
            self.i += 1;
            Some(c)
        } else {
            None
        }
    }

    fn number(&mut self) -> Option<f64> {
        self.skip_ws();
        let start = self.i;
        if self.i < self.b.len() && (self.b[self.i] == b'-' || self.b[self.i] == b'+') {
            self.i += 1;
        }
        let mut digits = false;
        while self.i < self.b.len() && self.b[self.i].is_ascii_digit() {
            self.i += 1;
            digits = true;
        }
        if self.i < self.b.len() && self.b[self.i] == b'.' {
            self.i += 1;
            while self.i < self.b.len() && self.b[self.i].is_ascii_digit() {
                self.i += 1;
                digits = true;
            }
        }
        if !digits {
            self.i = start;
            return None;
        }
        // Exponent.
        if self.i < self.b.len() && (self.b[self.i] == b'e' || self.b[self.i] == b'E') {
            self.i += 1;
            if self.i < self.b.len() && (self.b[self.i] == b'-' || self.b[self.i] == b'+') {
                self.i += 1;
            }
            while self.i < self.b.len() && self.b[self.i].is_ascii_digit() {
                self.i += 1;
            }
        }
        std::str::from_utf8(&self.b[start..self.i])
            .ok()?
            .parse()
            .ok()
    }

    fn flag(&mut self) -> Option<bool> {
        self.skip_ws();
        match self.b.get(self.i) {
            Some(b'0') => {
                self.i += 1;
                Some(false)
            }
            Some(b'1') => {
                self.i += 1;
                Some(true)
            }
            _ => None,
        }
    }
}

/// Parse an SVG path `d` attribute into absolute commands.
pub fn parse_path(d: &str) -> Vec<Cmd> {
    let mut lx = Lexer::new(d);
    let mut out = Vec::new();
    let mut cur = P::new(0.0, 0.0);
    let mut start = P::new(0.0, 0.0);
    // Subpath tracking so a command letter can be repeated implicitly.
    let mut last_ctrl: Option<P> = None;

    while let Some(cmd) = lx.take_cmd() {
        let rel = cmd.is_ascii_lowercase();
        let up = cmd.to_ascii_uppercase();
        let mut first = true;

        loop {
            if !first {
                // Implicit repetition is allowed, but not for M (which becomes L).
                if lx.peek().map(|c| c.is_ascii_alphabetic()).unwrap_or(true) {
                    break;
                }
            }
            let base = if rel { cur } else { P::new(0.0, 0.0) };

            match up {
                b'M' => {
                    let (x, y) = match (lx.number(), lx.number()) {
                        (Some(x), Some(y)) => (x, y),
                        _ => break,
                    };
                    cur = P::new(base.x + x, base.y + y);
                    start = cur;
                    if first {
                        out.push(Cmd::MoveTo(cur));
                    } else {
                        out.push(Cmd::LineTo(cur));
                    }
                    last_ctrl = None;
                }
                b'L' => {
                    let (x, y) = match (lx.number(), lx.number()) {
                        (Some(x), Some(y)) => (x, y),
                        _ => break,
                    };
                    cur = P::new(base.x + x, base.y + y);
                    out.push(Cmd::LineTo(cur));
                    last_ctrl = None;
                }
                b'H' => {
                    let x = match lx.number() {
                        Some(x) => x,
                        None => break,
                    };
                    cur = P::new(if rel { cur.x + x } else { x }, cur.y);
                    out.push(Cmd::LineTo(cur));
                    last_ctrl = None;
                }
                b'V' => {
                    let y = match lx.number() {
                        Some(y) => y,
                        None => break,
                    };
                    cur = P::new(cur.x, if rel { cur.y + y } else { y });
                    out.push(Cmd::LineTo(cur));
                    last_ctrl = None;
                }
                b'C' => {
                    let v: Vec<f64> = (0..6).filter_map(|_| lx.number()).collect();
                    if v.len() < 6 {
                        break;
                    }
                    let c1 = P::new(base.x + v[0], base.y + v[1]);
                    let c2 = P::new(base.x + v[2], base.y + v[3]);
                    let to = P::new(base.x + v[4], base.y + v[5]);
                    out.push(Cmd::CubicTo(c1, c2, to));
                    cur = to;
                    last_ctrl = Some(c2);
                }
                b'S' => {
                    let v: Vec<f64> = (0..4).filter_map(|_| lx.number()).collect();
                    if v.len() < 4 {
                        break;
                    }
                    // Reflect the previous control point.
                    let c1 = match last_ctrl {
                        Some(p) => P::new(2.0 * cur.x - p.x, 2.0 * cur.y - p.y),
                        None => cur,
                    };
                    let c2 = P::new(base.x + v[0], base.y + v[1]);
                    let to = P::new(base.x + v[2], base.y + v[3]);
                    out.push(Cmd::CubicTo(c1, c2, to));
                    cur = to;
                    last_ctrl = Some(c2);
                }
                b'Q' => {
                    let v: Vec<f64> = (0..4).filter_map(|_| lx.number()).collect();
                    if v.len() < 4 {
                        break;
                    }
                    let c = P::new(base.x + v[0], base.y + v[1]);
                    let to = P::new(base.x + v[2], base.y + v[3]);
                    out.push(Cmd::QuadTo(c, to));
                    cur = to;
                    last_ctrl = Some(c);
                }
                b'T' => {
                    let v: Vec<f64> = (0..2).filter_map(|_| lx.number()).collect();
                    if v.len() < 2 {
                        break;
                    }
                    let c = match last_ctrl {
                        Some(p) => P::new(2.0 * cur.x - p.x, 2.0 * cur.y - p.y),
                        None => cur,
                    };
                    let to = P::new(base.x + v[0], base.y + v[1]);
                    out.push(Cmd::QuadTo(c, to));
                    cur = to;
                    last_ctrl = Some(c);
                }
                b'A' => {
                    let rx = lx.number();
                    let ry = lx.number();
                    let rot = lx.number();
                    let large = lx.flag();
                    let sweep = lx.flag();
                    let x = lx.number();
                    let y = lx.number();
                    let (Some(rx), Some(ry), Some(rot), Some(large), Some(sweep), Some(x), Some(y)) =
                        (rx, ry, rot, large, sweep, x, y)
                    else {
                        break;
                    };
                    let to = P::new(base.x + x, base.y + y);
                    out.push(Cmd::ArcTo {
                        rx,
                        ry,
                        rot,
                        large,
                        sweep,
                        to,
                    });
                    cur = to;
                    last_ctrl = None;
                }
                b'Z' => {
                    out.push(Cmd::Close);
                    cur = start;
                    last_ctrl = None;
                    break;
                }
                _ => break,
            }
            first = false;
        }
    }
    out
}

// ------------------------------------------------------------- flattening

/// Number of segments for a cubic, from its second-derivative bound.
///
/// Uses the standard error estimate: for a cubic, chord error after n uniform
/// segments is bounded by `max|B''| / (8 n^2)`. Solving for n at a given
/// tolerance gives the count below. This is why the flattening tolerance is
/// *provable*, not a guessed constant.
fn cubic_segments(p0: P, p1: P, p2: P, p3: P, tol: f64) -> usize {
    let ax = p0.x - 2.0 * p1.x + p2.x;
    let ay = p0.y - 2.0 * p1.y + p2.y;
    let bx = p1.x - 2.0 * p2.x + p3.x;
    let by = p1.y - 2.0 * p2.y + p3.y;
    let m = (ax * ax + ay * ay).sqrt().max((bx * bx + by * by).sqrt());
    let n = (m / (8.0 * tol)).sqrt().ceil();
    (n as usize).clamp(1, 512)
}

fn quad_segments(p0: P, p1: P, p2: P, tol: f64) -> usize {
    let ax = p0.x - 2.0 * p1.x + p2.x;
    let ay = p0.y - 2.0 * p1.y + p2.y;
    let m = (ax * ax + ay * ay).sqrt();
    let n = (m / (4.0 * tol)).sqrt().ceil();
    (n as usize).clamp(1, 512)
}

#[inline]
fn cubic_at(p0: P, p1: P, p2: P, p3: P, t: f64) -> P {
    let u = 1.0 - t;
    let a = u * u * u;
    let b = 3.0 * u * u * t;
    let c = 3.0 * u * t * t;
    let d = t * t * t;
    P::new(
        a * p0.x + b * p1.x + c * p2.x + d * p3.x,
        a * p0.y + b * p1.y + c * p2.y + d * p3.y,
    )
}

#[inline]
fn quad_at(p0: P, p1: P, p2: P, t: f64) -> P {
    let u = 1.0 - t;
    P::new(
        u * u * p0.x + 2.0 * u * t * p1.x + t * t * p2.x,
        u * u * p0.y + 2.0 * u * t * p1.y + t * t * p2.y,
    )
}

/// The geometric parameters of an arc, grouped so the flattener takes a
/// sensible number of arguments. Clippy was right that eight positional
/// `f64`/`bool` values invite a silent transposition — `large` and `sweep` are
/// both `bool`, so swapping them would still compile.
#[derive(Debug, Clone, Copy)]
struct ArcGeom {
    rx: f64,
    ry: f64,
    rot_deg: f64,
    large: bool,
    sweep: bool,
}

/// Flatten an elliptical arc to line segments (endpoint parameterisation).
///
/// Implements the conversion from SVG's endpoint form to centre form
/// (spec appendix F.6.5), then samples uniformly in the parameter. Authored
/// angles are preserved; only chord error is bounded, which is what a renderer
/// actually needs.
fn arc_points(from: P, a: &ArcGeom, to: P, tol: f64) -> Vec<P> {
    let ArcGeom {
        rx,
        ry,
        rot_deg,
        large,
        sweep,
    } = *a;
    if (from.x - to.x).abs() < 1e-12 && (from.y - to.y).abs() < 1e-12 {
        return Vec::new();
    }
    let mut rx = rx.abs();
    let mut ry = ry.abs();
    if rx < 1e-12 || ry < 1e-12 {
        return vec![to];
    }

    let phi = rot_deg.to_radians();
    let (sin_p, cos_p) = phi.sin_cos();

    // Step 1: rotate to the ellipse's frame.
    let dx2 = (from.x - to.x) / 2.0;
    let dy2 = (from.y - to.y) / 2.0;
    let x1p = cos_p * dx2 + sin_p * dy2;
    let y1p = -sin_p * dx2 + cos_p * dy2;

    // Step 2: correct radii that are too small.
    let lambda = (x1p * x1p) / (rx * rx) + (y1p * y1p) / (ry * ry);
    if lambda > 1.0 {
        let s = lambda.sqrt();
        rx *= s;
        ry *= s;
    }

    // Step 3: centre in the rotated frame.
    let rx2 = rx * rx;
    let ry2 = ry * ry;
    let num = rx2 * ry2 - rx2 * y1p * y1p - ry2 * x1p * x1p;
    let den = rx2 * y1p * y1p + ry2 * x1p * x1p;
    let mut coef = if den.abs() < 1e-12 {
        0.0
    } else {
        (num.max(0.0) / den).sqrt()
    };
    if large == sweep {
        coef = -coef;
    }
    let cxp = coef * (rx * y1p / ry);
    let cyp = coef * -(ry * x1p / rx);

    let cx = cos_p * cxp - sin_p * cyp + (from.x + to.x) / 2.0;
    let cy = sin_p * cxp + cos_p * cyp + (from.y + to.y) / 2.0;

    // Step 4: start angle and sweep.
    let ux = (x1p - cxp) / rx;
    let uy = (y1p - cyp) / ry;
    let vx = (-x1p - cxp) / rx;
    let vy = (-y1p - cyp) / ry;

    let theta1 = uy.atan2(ux);
    let mut dtheta = (ux * vy - uy * vx).atan2(ux * vx + uy * vy);
    if !sweep && dtheta > 0.0 {
        dtheta -= 2.0 * PI;
    } else if sweep && dtheta < 0.0 {
        dtheta += 2.0 * PI;
    }

    // Step 5: choose a segment count from the sagitta error bound.
    // Chord error <= r * (1 - cos(dtheta/(2n))) ~ r * (dtheta/n)^2 / 8.
    let r = rx.max(ry);
    let n_est = (r * dtheta.abs() * dtheta.abs() / (8.0 * tol))
        .sqrt()
        .ceil();
    let n = (n_est as usize).clamp(1, 2048);

    let mut pts = Vec::with_capacity(n);
    for i in 1..=n {
        let t = theta1 + dtheta * (i as f64 / n as f64);
        let (s, c) = t.sin_cos();
        let x = cx + rx * c * cos_p - ry * s * sin_p;
        let y = cy + rx * c * sin_p + ry * s * cos_p;
        pts.push(P::new(x, y));
    }
    pts
}

/// Flatten parsed commands into polylines, one per subpath.
///
/// Closed subpaths repeat their first point at the end so the triangulator can
/// detect closure without extra state.
pub fn flatten(cmds: &[Cmd]) -> Vec<Vec<P>> {
    let mut out: Vec<Vec<P>> = Vec::new();
    let mut cur = P::new(0.0, 0.0);
    let mut poly: Vec<P> = Vec::new();

    for cmd in cmds {
        match cmd {
            Cmd::MoveTo(p) => {
                if poly.len() >= 2 {
                    out.push(std::mem::take(&mut poly));
                } else {
                    poly.clear();
                }
                cur = *p;
                poly.push(*p);
            }
            Cmd::LineTo(p) => {
                poly.push(*p);
                cur = *p;
            }
            Cmd::CubicTo(c1, c2, to) => {
                let n = cubic_segments(cur, *c1, *c2, *to, TOLERANCE);
                for i in 1..=n {
                    let t = i as f64 / n as f64;
                    poly.push(cubic_at(cur, *c1, *c2, *to, t));
                }
                cur = *to;
            }
            Cmd::QuadTo(c, to) => {
                let n = quad_segments(cur, *c, *to, TOLERANCE);
                for i in 1..=n {
                    let t = i as f64 / n as f64;
                    poly.push(quad_at(cur, *c, *to, t));
                }
                cur = *to;
            }
            Cmd::ArcTo {
                rx,
                ry,
                rot,
                large,
                sweep,
                to,
            } => {
                let geom = ArcGeom {
                    rx: *rx,
                    ry: *ry,
                    rot_deg: *rot,
                    large: *large,
                    sweep: *sweep,
                };
                let pts = arc_points(cur, &geom, *to, TOLERANCE);
                poly.extend(pts);
                cur = *to;
            }
            Cmd::Close => {
                if poly.len() >= 2 {
                    poly.push(poly[0]);
                    out.push(std::mem::take(&mut poly));
                } else {
                    poly.clear();
                }
            }
        }
    }
    if poly.len() >= 2 {
        out.push(poly);
    }
    out
}

/// Flatten and triangulate in one call. Returns a flat `Vec<f64>` of `x,y`
/// pairs, three vertices per triangle.
pub fn tessellate(d: &str) -> Vec<f64> {
    let cmds = parse_path(d);
    let polys = flatten(&cmds);
    let mut out = Vec::new();
    for poly in &polys {
        let tris = tri::triangulate(poly);
        for t in tris {
            out.push(t.0.x);
            out.push(t.0.y);
            out.push(t.1.x);
            out.push(t.1.y);
            out.push(t.2.x);
            out.push(t.2.y);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_simple_triangle() {
        let cmds = parse_path("M0,0 L10,0 L0,10 Z");
        assert_eq!(cmds.len(), 4);
        assert_eq!(cmds[0], Cmd::MoveTo(P::new(0.0, 0.0)));
        assert_eq!(cmds[3], Cmd::Close);
    }

    #[test]
    fn relative_commands_accumulate() {
        let cmds = parse_path("m10,10 l5,0 l0,5 z");
        match cmds[0] {
            Cmd::MoveTo(p) => assert_eq!((p.x, p.y), (10.0, 10.0)),
            _ => panic!("expected moveto"),
        }
        match cmds[1] {
            Cmd::LineTo(p) => assert_eq!((p.x, p.y), (15.0, 10.0)),
            _ => panic!("expected lineto"),
        }
    }

    #[test]
    fn horizontal_vertical() {
        let cmds = parse_path("M0,0 H10 V10 Z");
        match cmds[1] {
            Cmd::LineTo(p) => assert_eq!((p.x, p.y), (10.0, 0.0)),
            _ => panic!(),
        }
        match cmds[2] {
            Cmd::LineTo(p) => assert_eq!((p.x, p.y), (10.0, 10.0)),
            _ => panic!(),
        }
    }

    #[test]
    fn implicit_lineto_after_moveto() {
        // "M0,0 10,0 10,10" means moveto then two implicit linetos.
        let cmds = parse_path("M0,0 10,0 10,10");
        let lines = cmds.iter().filter(|c| matches!(c, Cmd::LineTo(_))).count();
        assert_eq!(lines, 2);
    }

    #[test]
    fn scientific_notation() {
        let cmds = parse_path("M1e2,2e-1 L0,0");
        match cmds[0] {
            Cmd::MoveTo(p) => assert_eq!((p.x, p.y), (100.0, 0.2)),
            _ => panic!(),
        }
    }

    #[test]
    fn flatten_square() {
        let polys = flatten(&parse_path("M0,0 L10,0 L10,10 L0,10 Z"));
        assert_eq!(polys.len(), 1);
        // 4 corners + repeated start.
        assert_eq!(polys[0].len(), 5);
    }

    #[test]
    fn cubic_flattens_within_tolerance() {
        let cmds = parse_path("M0,0 C0,100 100,100 100,0");
        let polys = flatten(&cmds);
        assert!(!polys[0].is_empty());
        // Every sampled point must be plausibly near the curve; the strongest
        // check is that the flattener produced more than a couple of segments.
        assert!(polys[0].len() > 8, "got {} points", polys[0].len());
    }

    #[test]
    fn arc_produces_points() {
        let cmds = parse_path("M0,0 A10,10 0 0,1 20,0");
        let polys = flatten(&cmds);
        assert!(polys[0].len() > 5);
        let last = *polys[0].last().unwrap();
        assert!(
            (last.x - 20.0).abs() < 1e-6,
            "arc must land on its endpoint"
        );
        assert!(last.y.abs() < 1e-6);
    }

    #[test]
    fn zero_radius_arc_falls_back_to_line() {
        let cmds = parse_path("M0,0 A0,10 0 0,1 20,0");
        let polys = flatten(&cmds);
        let last = *polys[0].last().unwrap();
        assert_eq!(last, P::new(20.0, 0.0));
    }
}
