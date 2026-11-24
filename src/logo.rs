//! App logo: the likeness template as a mad scientist, with a lab coat and wild grey hair
//! around a bald top. It uses the same 740x700 design space and `Pen` as avatar.rs. Ink
//! lines are drawn ragged, like the hand-drawn original. `icon` puts the figure on the
//! light blue macOS icon tile (1024x1024 space).

use crate::avatar::{arc, circle, closed, Pen, Rgb, Seg, Seg::*};
use std::f64::consts::PI;

pub const INK: Rgb = (0.07, 0.07, 0.07);
const LINE: f64 = 3.2;
const WHITE: Rgb = (1.0, 1.0, 1.0);

fn hex(c: u32) -> Rgb {
    (((c >> 16) & 255) as f64 / 255.0, ((c >> 8) & 255) as f64 / 255.0, (c & 255) as f64 / 255.0)
}
fn grey(v: u8) -> Rgb {
    let g = v as f64 / 255.0;
    (g, g, g)
}

/// Small deterministic generator so the hair and the line wobble come out the same every time.
struct Rng(u64);
impl Rng {
    fn new(seed: u64) -> Rng {
        Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1)
    }
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }
    fn f(&mut self) -> f64 {
        (self.next() >> 11) as f64 / (1u64 << 53) as f64
    }
    fn range(&mut self, a: f64, b: f64) -> f64 {
        a + (b - a) * self.f()
    }
    fn int(&mut self, a: usize, b: usize) -> usize {
        a + (self.next() % (b - a + 1) as u64) as usize
    }
    fn pick<T: Copy>(&mut self, v: &[T]) -> T {
        v[self.int(0, v.len() - 1)]
    }
}

type Pt = (f64, f64);

fn dist(a: Pt, b: Pt) -> f64 {
    (b.0 - a.0).hypot(b.1 - a.1)
}

/// Path to points; curves are cut into `steps` pieces.
fn flatten(path: &[Seg], steps: usize) -> Vec<Pt> {
    let mut pts: Vec<Pt> = vec![];
    let mut cur = (0.0, 0.0);
    for s in path {
        match *s {
            M(x, y) | L(x, y) => {
                cur = (x, y);
                pts.push(cur);
            }
            Q(a, b, x, y) => {
                for i in 1..=steps {
                    let t = i as f64 / steps as f64;
                    let u = 1.0 - t;
                    pts.push((u * u * cur.0 + 2.0 * u * t * a + t * t * x, u * u * cur.1 + 2.0 * u * t * b + t * t * y));
                }
                cur = (x, y);
            }
            C(a, b, c, d, x, y) => {
                for i in 1..=steps {
                    let t = i as f64 / steps as f64;
                    let u = 1.0 - t;
                    let (k0, k1, k2, k3) = (u * u * u, 3.0 * u * u * t, 3.0 * u * t * t, t * t * t);
                    pts.push((k0 * cur.0 + k1 * a + k2 * c + k3 * x, k0 * cur.1 + k1 * b + k2 * d + k3 * y));
                }
                cur = (x, y);
            }
            Z => {}
        }
    }
    pts
}

fn polyline(pts: &[Pt]) -> Vec<Seg> {
    pts.iter().enumerate().map(|(i, &(x, y))| if i == 0 { M(x, y) } else { L(x, y) }).collect()
}

struct Logo<'a> {
    p: &'a mut dyn Pen,
    inked: u64,
}

impl Logo<'_> {
    fn fill(&mut self, path: &[Seg], c: Rgb) {
        self.p.fill(path, c);
    }
    fn stroke(&mut self, path: &[Seg], c: Rgb, w: f64) {
        self.p.stroke(path, c, w);
    }

    /// Ink line the way the original was drawn: a slight wobble along the line and uneven
    /// pressure, as short overlapping pieces of slightly different width.
    fn ink(&mut self, path: &[Seg], w: f64) {
        self.inked += 1;
        let mut r = Rng::new(self.inked * 7919);
        let mut pts = flatten(path, 16);
        if path.iter().any(|s| matches!(s, Z)) {
            pts.push(pts[0]);
        }
        let mut even = vec![pts[0]];
        let mut acc = 0.0;
        for i in 1..pts.len() {
            acc += dist(pts[i - 1], pts[i]);
            if acc >= 2.5 || i == pts.len() - 1 {
                even.push(pts[i]);
                acc = 0.0;
            }
        }
        if even.len() < 3 {
            self.p.stroke(path, INK, w);
            return;
        }
        let (p1, p2, p3) = (r.range(0.0, 6.3), r.range(0.0, 6.3), r.range(0.0, 6.3));
        let (l1, l2) = (r.range(28.0, 46.0), r.range(9.0, 15.0));
        let mut moved = Vec::with_capacity(even.len());
        let mut s = 0.0;
        for i in 0..even.len() {
            let (a, b) = (even[i.saturating_sub(1)], even[(i + 1).min(even.len() - 1)]);
            let (tx, ty) = (b.0 - a.0, b.1 - a.1);
            let m = match tx.hypot(ty) { m if m > 0.0 => m, _ => 1.0 };
            let (nx, ny) = (-ty / m, tx / m);
            if i > 0 {
                s += dist(even[i - 1], even[i]);
            }
            let off = 0.9 * (s / l1 + p1).sin() + 0.45 * (s / l2 + p2).sin() + 0.2 * (s / 4.3 + p3).sin();
            moved.push((even[i].0 + nx * off, even[i].1 + ny * off));
        }
        let mut i = 0;
        while i + 1 < moved.len() {
            let n = r.int(5, 12);
            let end = (i + n).min(moved.len() - 1);
            self.p.stroke(&polyline(&moved[i..=end]), INK, w * r.range(0.82, 1.18));
            i += n;
        }
    }
}

const HEAD: (f64, f64, f64, f64) = (330.0, 240.0, 188.0, 170.0);

/// Point on the head ellipse at angle `a` (degrees), scaled by `k` from the centre.
fn on_head(a: f64, k: f64) -> Pt {
    let (hx, hy, rx, ry) = HEAD;
    let a = a.to_radians();
    (hx + rx * k * a.cos(), hy + ry * k * a.sin())
}
/// Inverse of `on_head`: (angle in degrees, k).
fn polar(p: Pt) -> (f64, f64) {
    let (hx, hy, rx, ry) = HEAD;
    let (u, v) = ((p.0 - hx) / rx, (p.1 - hy) / ry);
    (v.atan2(u).to_degrees(), u.hypot(v))
}

#[derive(Clone, Copy)]
enum Part {
    /// rounded lump: height, skew (degrees)
    Bump(f64, f64),
    /// pointed flyaway lick: length, lean (degrees)
    Flick(f64, f64),
}

/// Outline of one clump of hair over the side of the head. `valleys` are the angles between
/// the parts, from one end of the clump to the other.
fn tuft(valleys: &[f64], parts: &[Part], inner: f64) -> Vec<Seg> {
    let p0 = on_head(valleys[0], 0.95);
    let mut v = vec![M(p0.0, p0.1)];
    for (i, part) in parts.iter().enumerate() {
        let (a0, a1) = (valleys[i], valleys[i + 1]);
        let last = i + 1 == parts.len();
        let v1 = on_head(a1, if last { 0.95 } else { 1.06 + 0.04 * ((i * 5) % 3) as f64 });
        match *part {
            Part::Bump(h, t) => {
                let (c1, c2) = (on_head(a0 + t, h), on_head(a1 + t, h));
                v.push(C(c1.0, c1.1, c2.0, c2.1, v1.0, v1.1));
            }
            Part::Flick(h, t) => {
                let tip = (a0 + a1) / 2.0 + t;
                let c1 = on_head((a0 + tip) / 2.0 + t * 0.35, 1.0 + (h - 1.0) * 0.55);
                let c2 = on_head((tip + a1) / 2.0 + t * 0.35, 1.0 + (h - 1.0) * 0.55);
                let tp = on_head(tip, h);
                v.push(Q(c1.0, c1.1, tp.0, tp.1));
                v.push(Q(c2.0, c2.1, v1.0, v1.1));
            }
        }
    }
    // back along the inside of the head, dipping in a little over the temple
    let (first, end) = (valleys[0], valleys[valleys.len() - 1]);
    for i in 1..=18 {
        let f = i as f64 / 18.0;
        let p = on_head(end + (first - end) * f, 0.95 - (0.95 - inner) * (PI * f).sin());
        v.push(L(p.0, p.1));
    }
    v.push(Z);
    v
}

/// A clump: soft grey body, then strands growing out of the scalp to the clump's outline,
/// so the edge is made of hair ends rather than a line.
fn hair_clump(lg: &mut Logo, valleys: &[f64], parts: &[Part], seed: u64) {
    let outline = flatten(&tuft(valleys, parts, 0.93), 10);
    let body: Vec<Pt> = outline
        .iter()
        .map(|&p| {
            let (a, k) = polar(p);
            on_head(a, if k > 1.0 { 1.0 + (k - 1.0) * 0.9 } else { k })
        })
        .collect();
    let mut body = polyline(&body);
    body.push(Z);
    lg.fill(&body, grey(0xd4));

    // angles measured around the clump's middle, so the left side doesn't wrap at 180
    let mid = (valleys[0] + valleys[valleys.len() - 1]) / 2.0;
    let pol: Vec<(f64, f64)> = outline
        .iter()
        .map(|&p| {
            let (a, k) = polar(p);
            ((a - mid + 180.0).rem_euclid(360.0) - 180.0 + mid, k)
        })
        .collect();
    let outer = pol.iter().filter(|q| q.1 > 1.02).map(|q| q.0);
    let lo = outer.clone().fold(f64::MAX, f64::min) - 2.0;
    let hi = outer.fold(f64::MIN, f64::max) + 2.0;
    let reach = |a: f64| pol.iter().filter(|q| (q.0 - a).abs() < 4.0).fold(1.0f64, |m, q| m.max(q.1));

    let shades = [grey(0xa6), grey(0xb2), grey(0xbe), grey(0xc8)];
    let mut r = Rng::new(seed);
    for _ in 0..170 {
        let a = r.range(lo + 3.0, hi - 3.0);
        let lean = r.range(-4.0, 4.0);
        let k0 = r.range(0.94, 1.0);
        let k1 = (reach(a) * r.range(0.9, 1.03)).max(1.04);
        let (p0, p1) = (on_head(a, k0), on_head(a + lean, k1));
        let c = on_head(a + lean * 0.3 + r.range(-4.0, 4.0), (k0 + k1) / 2.0);
        let col = r.pick(&shades);
        lg.stroke(&[M(p0.0, p0.1), Q(c.0, c.1, p1.0, p1.1)], col, r.range(2.2, 3.2));
    }
}

/// Bushy brow: a solid tapered core with short hairs on it, growing toward the outer end
/// and tilted up. `c0` is the thick inner end.
fn brow(lg: &mut Logo, c0: Pt, ctl: Pt, c1: Pt, thick: (f64, f64), seed: u64) {
    let at = |t: f64| {
        let u = 1.0 - t;
        let p = (u * u * c0.0 + 2.0 * u * t * ctl.0 + t * t * c1.0, u * u * c0.1 + 2.0 * u * t * ctl.1 + t * t * c1.1);
        let d = (2.0 * u * (ctl.0 - c0.0) + 2.0 * t * (c1.0 - ctl.0), 2.0 * u * (ctl.1 - c0.1) + 2.0 * t * (c1.1 - ctl.1));
        let m = d.0.hypot(d.1);
        (p, (d.0 / m, d.1 / m))
    };
    let width = |t: f64| thick.0 + (thick.1 - thick.0) * t;

    let (mut top, mut bot) = (vec![], vec![]);
    for i in 0..=20 {
        let t = i as f64 / 20.0;
        let ((x, y), _) = at(t);
        let th = width(t) * 0.42 * (0.2 + 0.8 * (t / 0.16).min(1.0).sqrt());
        top.push((x, y - th));
        bot.push((x, y + th * 0.8));
    }
    top.extend(bot.into_iter().rev());
    let mut core = polyline(&top);
    core.push(Z);
    lg.fill(&core, grey(0x9a));

    let shades = [grey(0x7c), grey(0x8a), grey(0x98), grey(0xa8)];
    let mut r = Rng::new(seed);
    for _ in 0..110 {
        let t = r.f().powf(0.8);
        let ((x, y), (tx, ty)) = at(t);
        let (nx, ny) = if -tx > 0.0 { (-ty, tx) } else { (ty, -tx) }; // the side facing up
        let off = r.range(-0.4, 0.4) * width(t);
        let (bx, by) = (x + nx * off, y + ny * off);
        let tilt = 0.55 + r.range(-0.35, 0.35);
        let (dx, dy) = (tx * tilt.cos() + nx * tilt.sin(), ty * tilt.cos() + ny * tilt.sin());
        let ln = r.range(7.0, 13.0) * (1.1 - 0.4 * t);
        let (p0, p1) = ((bx - dx * ln * 0.5, by - dy * ln * 0.5), (bx + dx * ln * 0.5, by + dy * ln * 0.5));
        let c = ((p0.0 + p1.0) / 2.0 + nx * 2.0, (p0.1 + p1.1) / 2.0 + ny * 2.0);
        let col = r.pick(&shades);
        lg.stroke(&[M(p0.0, p0.1), Q(c.0, c.1, p1.0, p1.1)], col, r.range(1.8, 2.8));
    }
}

/// One stray hair on the bald top.
fn wisp(lg: &mut Logo, pts: &[Pt]) {
    let mut v = vec![M(pts[0].0, pts[0].1)];
    for i in (1..pts.len() - 1).step_by(2) {
        v.push(Q(pts[i].0, pts[i].1, pts[i + 1].0, pts[i + 1].1));
    }
    lg.stroke(&v, grey(0xa8), 3.2);
}

/// The figure in design space (same space and VIEW as the likeness).
pub fn draw(p: &mut dyn Pen) {
    let lg = &mut Logo { p, inked: 0 };
    let blue = hex(0x1e9fe8);

    // ---- body: white lab coat, outlined in open strokes like the template
    let body = [
        M(252.0, 393.0), Q(220.0, 396.0, 203.0, 410.0), C(165.0, 440.0, 155.0, 500.0, 158.0, 548.0), C(160.0, 605.0, 168.0, 645.0, 212.0, 664.0),
        Q(300.0, 638.0, 394.0, 608.0), Q(400.0, 621.0, 430.0, 622.0), Q(500.0, 620.0, 560.0, 617.0),
        C(596.0, 610.0, 606.0, 582.0, 600.0, 545.0), C(592.0, 480.0, 572.0, 425.0, 548.0, 392.0),
        Q(515.0, 372.0, 455.0, 372.0), L(330.0, 330.0), Z,
    ];
    lg.fill(&body, WHITE);

    // tie in the V of the coat
    let tie = [M(330.0, 408.0), L(350.0, 408.0), L(356.0, 430.0), L(368.0, 520.0), L(344.0, 542.0), L(330.0, 542.0), L(326.0, 520.0), L(336.0, 430.0), Z];
    lg.fill(&tie, blue);
    lg.ink(&tie, LINE * 0.7);
    lg.ink(&[M(336.0, 430.0), L(356.0, 430.0)], LINE * 0.7);

    // lapels: collar point, notch, then down to where the arms cross
    lg.ink(&[M(262.0, 396.0), Q(246.0, 418.0, 228.0, 436.0), L(262.0, 452.0), L(252.0, 466.0), Q(286.0, 506.0, 320.0, 544.0)], LINE * 0.85);
    lg.ink(&[M(432.0, 384.0), Q(452.0, 404.0, 470.0, 420.0), L(438.0, 440.0), L(450.0, 452.0), Q(404.0, 498.0, 366.0, 542.0)], LINE * 0.85);

    // breast pocket with two pens
    for (x, c) in [(478.0, blue), (494.0, hex(0xe0453a))] {
        let pen = [M(x - 5.0, 458.0), L(x - 5.0, 486.0), L(x + 5.0, 486.0), L(x + 5.0, 458.0), Q(x, 448.0, x - 5.0, 458.0), Z];
        lg.fill(&pen, c);
        lg.ink(&pen, LINE * 0.6);
    }
    lg.fill(&[M(462.0, 474.0), Q(488.0, 470.0, 516.0, 472.0), L(518.0, 512.0), Q(490.0, 516.0, 464.0, 514.0), Z], WHITE);
    lg.ink(&[M(462.0, 474.0), Q(488.0, 470.0, 516.0, 472.0), L(518.0, 510.0), Q(490.0, 516.0, 464.0, 514.0), L(462.0, 474.0)], LINE * 0.75);

    lg.ink(&[M(252.0, 393.0), Q(220.0, 396.0, 203.0, 410.0), C(165.0, 440.0, 155.0, 500.0, 158.0, 548.0), C(160.0, 605.0, 168.0, 645.0, 212.0, 664.0)], LINE);
    lg.ink(&[M(212.0, 664.0), Q(322.0, 632.0, 442.0, 592.0)], LINE);
    lg.ink(&[M(455.0, 372.0), Q(515.0, 372.0, 548.0, 392.0), C(576.0, 440.0, 592.0, 500.0, 599.0, 550.0), C(605.0, 592.0, 590.0, 614.0, 560.0, 617.0), Q(480.0, 621.0, 410.0, 621.0)], LINE);
    lg.ink(&[M(585.0, 548.0), Q(600.0, 572.0, 592.0, 602.0)], LINE * 0.7);
    // crossed forearm and the mitten hand, closed underneath like the original
    lg.ink(&[M(306.0, 548.0), Q(372.0, 537.0, 444.0, 540.0)], LINE);
    lg.ink(&[M(306.0, 550.0), Q(300.0, 521.0, 262.0, 520.0), Q(224.0, 522.0, 212.0, 553.0), Q(205.0, 578.0, 213.0, 587.0), Q(226.0, 592.0, 242.0, 578.0), Q(270.0, 558.0, 306.0, 552.0)], LINE);
    lg.ink(&[M(229.0, 586.0), L(236.0, 571.0)], LINE * 0.7);

    // ---- head: the outline shows on the bald top
    let (hx, hy, rx, ry) = HEAD;
    lg.fill(&closed(arc(hx, hy, rx, ry, 0.0, 360.0, 0.008)), WHITE);
    lg.ink(&arc(hx, hy, rx, ry, -110.0, 228.0, 0.008), LINE);
    lg.ink(&[M(282.0, 150.0), Q(330.0, 138.0, 380.0, 150.0)], LINE * 0.7);
    lg.ink(&[M(296.0, 128.0), Q(330.0, 120.0, 364.0, 128.0)], LINE * 0.7);

    // ---- wild grey hair at both sides, a few stray hairs on top
    use Part::{Bump, Flick};
    let left_v = [146.0, 158.0, 170.0, 183.0, 195.0, 208.0, 221.0, 236.0];
    let left = [Bump(1.26, -4.0), Flick(1.48, -10.0), Bump(1.34, 0.0), Bump(1.40, 3.0), Flick(1.56, 8.0), Bump(1.30, 4.0), Flick(1.36, 14.0)];
    let right_v = [34.0, 22.0, 10.0, -3.0, -16.0, -29.0, -42.0, -56.0];
    let right = [Flick(1.36, -1.0), Bump(1.32, 0.0), Bump(1.38, -2.0), Flick(1.60, -6.0), Bump(1.34, -3.0), Flick(1.46, -14.0), Bump(1.22, -4.0)];
    hair_clump(lg, &left_v, &left, 11);
    hair_clump(lg, &right_v, &right, 12);
    wisp(lg, &[(312.0, 74.0), (300.0, 46.0), (318.0, 30.0), (334.0, 18.0), (322.0, 6.0)]);
    wisp(lg, &[(346.0, 72.0), (352.0, 44.0), (372.0, 36.0), (390.0, 30.0), (392.0, 12.0)]);
    wisp(lg, &[(282.0, 82.0), (262.0, 58.0), (246.0, 54.0)]);
    wisp(lg, &[(330.0, 70.0), (326.0, 40.0), (338.0, 26.0)]);
    wisp(lg, &[(364.0, 76.0), (380.0, 56.0), (398.0, 58.0)]);

    // ---- eyes, as in the template
    for (eye, ix, iy, ir) in [
        ([M(183.0, 258.0), Q(236.0, 204.0, 294.0, 252.0), Q(246.0, 302.0, 183.0, 258.0), Z], 245.0, 254.0, 25.0),
        ([M(354.0, 252.0), Q(410.0, 198.0, 471.0, 245.0), Q(416.0, 296.0, 354.0, 252.0), Z], 414.0, 248.0, 24.0),
    ] {
        lg.fill(&eye, WHITE);
        lg.fill(&circle(ix, iy, ir), blue);
        lg.stroke(&arc(ix, iy, ir, ir, 0.0, 360.0, 0.0), hex(0x11598a), 2.0);
        lg.fill(&circle(ix + 4.0, iy - 5.0, 8.0), INK);
        lg.fill(&circle(ix - 11.0, iy - 10.0, 4.5), WHITE);
        lg.ink(&eye, LINE * 0.8);
    }

    brow(lg, (290.0, 200.0), (242.0, 182.0), (194.0, 216.0), (30.0, 13.0), 101);
    brow(lg, (368.0, 196.0), (420.0, 176.0), (468.0, 210.0), (30.0, 13.0), 102);

    // ---- smug half-smile
    lg.ink(&[M(280.0, 351.0), Q(286.0, 362.0, 302.0, 362.0), Q(366.0, 362.0, 409.0, 336.0)], LINE * 0.9);
    lg.ink(&[M(404.0, 341.0), L(412.0, 330.0)], LINE * 0.9);
}

/// Moves and scales everything drawn through it.
struct Placed<'a> {
    p: &'a mut dyn Pen,
    s: f64,
    dx: f64,
    dy: f64,
}
impl Placed<'_> {
    fn map(&self, path: &[Seg]) -> Vec<Seg> {
        let f = |x: f64, y: f64| (self.dx + x * self.s, self.dy + y * self.s);
        path.iter()
            .map(|seg| match *seg {
                M(x, y) => { let p = f(x, y); M(p.0, p.1) }
                L(x, y) => { let p = f(x, y); L(p.0, p.1) }
                Q(a, b, x, y) => { let (k, p) = (f(a, b), f(x, y)); Q(k.0, k.1, p.0, p.1) }
                C(a, b, c, d, x, y) => { let (k1, k2, p) = (f(a, b), f(c, d), f(x, y)); C(k1.0, k1.1, k2.0, k2.1, p.0, p.1) }
                Z => Z,
            })
            .collect()
    }
}
impl Pen for Placed<'_> {
    fn fill(&mut self, path: &[Seg], c: Rgb) {
        let v = self.map(path);
        self.p.fill(&v, c);
    }
    fn stroke(&mut self, path: &[Seg], c: Rgb, w: f64) {
        let v = self.map(path);
        self.p.stroke(&v, c, w * self.s);
    }
}

/// Rounded square from (x, y), side `w`, corner radius `r`.
fn tile(x: f64, y: f64, w: f64, r: f64) -> Vec<Seg> {
    let k = r * 0.5523; // circle-ish Bezier corners
    let (x1, y1) = (x + w, y + w);
    vec![
        M(x + r, y), L(x1 - r, y), C(x1 - r + k, y, x1, y + r - k, x1, y + r),
        L(x1, y1 - r), C(x1, y1 - r + k, x1 - r + k, y1, x1 - r, y1),
        L(x + r, y1), C(x + r - k, y1, x, y1 - r + k, x, y1 - r),
        L(x, y + r), C(x, y + r - k, x + r - k, y, x + r, y), Z,
    ]
}

/// The app icon in a 1024x1024 space: the figure on a light blue tile, with the usual
/// transparent margin around it.
pub fn icon(p: &mut dyn Pen) {
    let t = tile(100.0, 100.0, 824.0, 185.0);
    p.fill(&t, hex(0xd8eefb));
    let s = 1.14;
    draw(&mut Placed { p: &mut *p, s, dx: 512.0 - 340.0 * s, dy: 128.0 - 4.0 * s });
    p.stroke(&tile(101.5, 101.5, 821.0, 183.5), hex(0xb5d9ef), 3.0);
}

/// The icon as SVG (1024x1024).
pub fn icon_svg() -> String {
    let mut pen = crate::avatar::Svg(String::new());
    icon(&mut pen);
    format!("<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 1024 1024\" width=\"1024\" height=\"1024\">\n{}</svg>\n", pen.0)
}
