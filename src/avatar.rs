//! Likeness avatar in the hand-drawn "stickman" style: round head, thick black
//! outline with small gaps, almond eyes with a coloured iris, smug half-smile,
//! blob body with crossed arms. Every trait is a vector layer that is recoloured
//! at draw time, so there are no image assets at all.
//!
//! Geometry lives in a fixed 740x700 design space (the original template's size);
//! backends scale it. Two backends: Core Graphics (gui.rs) and SVG (CLI previews).

use crate::traits::{Sex, Texture, Traits};

#[derive(Clone, Copy, Debug)]
pub enum Seg {
    M(f64, f64),
    L(f64, f64),
    Q(f64, f64, f64, f64),
    C(f64, f64, f64, f64, f64, f64),
    Z,
}
use Seg::*;

pub type Rgb = (f64, f64, f64);

pub trait Pen {
    fn fill(&mut self, path: &[Seg], c: Rgb);
    fn stroke(&mut self, path: &[Seg], c: Rgb, width: f64);
}

/// Everything the avatar needs; built from `Traits` or by hand for previews.
#[derive(Clone, Debug)]
pub struct Spec {
    pub skin: [u8; 3],
    pub hair: [u8; 3],
    pub eyes: [u8; 3],
    pub texture: Texture,
    pub long_hair: bool,
    pub freckles: u8,
}

impl Spec {
    pub fn from_traits(t: &Traits) -> Spec {
        Spec { skin: t.skin_rgb, hair: t.hair_rgb, eyes: t.eye_rgb, texture: t.texture, long_hair: t.sex == Sex::Female, freckles: t.freckles }
    }
}

/// Visible bounds of the drawing in design space: (x, y, w, h).
pub const VIEW: (f64, f64, f64, f64) = (115.0, 30.0, 510.0, 660.0);

const INK: Rgb = (0.07, 0.07, 0.07);
const LINE: f64 = 4.5;
const WHITE: Rgb = (1.0, 1.0, 1.0);

fn rgb(c: [u8; 3], f: f64) -> Rgb {
    ((c[0] as f64 / 255.0 * f).min(1.0), (c[1] as f64 / 255.0 * f).min(1.0), (c[2] as f64 / 255.0 * f).min(1.0))
}

/// Ellipse arc as a polyline, with a slight deterministic wobble so it reads hand-drawn.
pub(crate) fn arc(cx: f64, cy: f64, rx: f64, ry: f64, a0: f64, a1: f64, wobble: f64) -> Vec<Seg> {
    let n = 72;
    (0..=n)
        .map(|i| {
            let a = (a0 + (a1 - a0) * i as f64 / n as f64).to_radians();
            let k = 1.0 + wobble * (3.0 * a + 1.3).sin() + wobble * 0.5 * (7.0 * a).sin();
            let p = (cx + rx * k * a.cos(), cy + ry * k * a.sin());
            if i == 0 { M(p.0, p.1) } else { L(p.0, p.1) }
        })
        .collect()
}
pub(crate) fn closed(mut v: Vec<Seg>) -> Vec<Seg> {
    v.push(Z);
    v
}
pub(crate) fn circle(cx: f64, cy: f64, r: f64) -> Vec<Seg> {
    closed(arc(cx, cy, r, r, 0.0, 360.0, 0.0))
}

const HEAD: (f64, f64, f64, f64) = (330.0, 240.0, 188.0, 170.0);

pub fn draw(p: &mut dyn Pen, s: &Spec) {
    let skin = rgb(s.skin, 1.0);
    let hair = rgb(s.hair, 1.0);
    // strand lines: darker than the hair, or lighter when the hair is near-black
    let lum = 0.3 * s.hair[0] as f64 + 0.59 * s.hair[1] as f64 + 0.11 * s.hair[2] as f64;
    let hair_line = if lum < 55.0 { let h = rgb(s.hair, 1.0); (h.0 + 0.2, h.1 + 0.2, h.2 + 0.2) } else { rgb(s.hair, 0.6) };

    // ---- long hair behind everything (women); the body later covers the middle
    if s.long_hair {
        match s.texture {
            Texture::Straight => {
                let back = [M(150.0, 180.0), L(130.0, 470.0), L(530.0, 470.0), L(510.0, 180.0), Z];
                p.fill(&back, hair);
                p.stroke(&back, INK, LINE);
                for &x in &[146.0, 514.0] {
                    p.stroke(&[M(x, 300.0), L(x - (x - 330.0) * 0.03, 460.0)], hair_line, 2.5);
                }
            }
            Texture::Wavy => {
                let back = [
                    M(150.0, 180.0), Q(108.0, 250.0, 130.0, 320.0), Q(150.0, 390.0, 122.0, 440.0), Q(108.0, 480.0, 150.0, 486.0),
                    L(510.0, 486.0), Q(552.0, 480.0, 538.0, 440.0), Q(510.0, 390.0, 530.0, 320.0), Q(552.0, 250.0, 510.0, 180.0), Z,
                ];
                p.fill(&back, hair);
                p.stroke(&back, INK, LINE);
                p.stroke(&[M(140.0, 330.0), Q(158.0, 385.0, 136.0, 440.0)], hair_line, 2.5);
                p.stroke(&[M(520.0, 330.0), Q(502.0, 385.0, 524.0, 440.0)], hair_line, 2.5);
            }
            Texture::Curly => {
                p.fill(&[M(150.0, 190.0), L(118.0, 480.0), L(542.0, 480.0), L(510.0, 190.0), Z], hair);
                let mut curls = vec![];
                for i in 0..6 {
                    let y = 220.0 + 50.0 * i as f64;
                    let dx = if i % 2 == 0 { 0.0 } else { 10.0 };
                    curls.push((138.0 + dx, y));
                    curls.push((522.0 - dx, y));
                }
                for &(x, y) in &curls {
                    p.fill(&circle(x, y, 36.0), hair);
                    p.stroke(&arc(x, y, 36.0, 36.0, 0.0, 360.0, 0.03), INK, LINE * 0.85);
                    p.stroke(&arc(x + 2.0, y + 2.0, 16.0, 16.0, 200.0, 470.0, 0.0), hair_line, 2.5);
                }
            }
        }
    }

    // ---- body (filled as one blob, outlined in open strokes like the template)
    let body = [
        M(252.0, 393.0), Q(220.0, 396.0, 203.0, 410.0), C(165.0, 440.0, 155.0, 500.0, 158.0, 548.0), C(160.0, 605.0, 168.0, 645.0, 212.0, 664.0),
        Q(300.0, 638.0, 394.0, 608.0), Q(400.0, 621.0, 430.0, 622.0), Q(500.0, 620.0, 560.0, 617.0),
        C(596.0, 610.0, 606.0, 582.0, 600.0, 545.0), C(592.0, 480.0, 572.0, 425.0, 548.0, 392.0),
        Q(515.0, 372.0, 455.0, 372.0), L(330.0, 330.0), Z, // top edge tucked under the head
    ];
    p.fill(&body, skin);
    p.stroke(&[M(252.0, 393.0), Q(220.0, 396.0, 203.0, 410.0), C(165.0, 440.0, 155.0, 500.0, 158.0, 548.0), C(160.0, 605.0, 168.0, 645.0, 212.0, 664.0)], INK, LINE);
    p.stroke(&[M(212.0, 664.0), Q(322.0, 632.0, 442.0, 592.0)], INK, LINE);
    p.stroke(&[M(455.0, 372.0), Q(515.0, 372.0, 548.0, 392.0), C(576.0, 440.0, 592.0, 500.0, 599.0, 550.0), C(605.0, 592.0, 590.0, 614.0, 560.0, 617.0), Q(500.0, 620.0, 430.0, 622.0), Q(400.0, 621.0, 394.0, 608.0)], INK, LINE);
    p.stroke(&[M(585.0, 548.0), Q(600.0, 572.0, 592.0, 602.0)], INK, LINE * 0.7);
    // crossed forearm + hand
    p.stroke(&[M(306.0, 548.0), Q(372.0, 537.0, 444.0, 540.0)], INK, LINE);
    p.stroke(&[M(306.0, 550.0), Q(300.0, 521.0, 262.0, 520.0), Q(224.0, 522.0, 212.0, 553.0), Q(205.0, 578.0, 213.0, 587.0), Q(226.0, 592.0, 240.0, 578.0)], INK, LINE);
    p.stroke(&[M(229.0, 586.0), L(236.0, 571.0)], INK, LINE * 0.7);

    // ---- head
    let (hx, hy, rx, ry) = HEAD;
    p.fill(&closed(arc(hx, hy, rx, ry, 0.0, 360.0, 0.008)), skin);
    p.stroke(&arc(hx, hy, rx, ry, -110.0, 228.0, 0.008), INK, LINE);

    // ---- freckles (under the hair so a fringe can cover nothing important)
    let spots = [(212.0, 312.0), (232.0, 322.0), (198.0, 326.0), (444.0, 306.0), (424.0, 318.0), (460.0, 322.0), (220.0, 336.0), (440.0, 330.0), (248.0, 308.0), (410.0, 304.0), (206.0, 300.0), (452.0, 292.0), (238.0, 340.0), (426.0, 336.0)];
    let fc = rgb(s.skin, 0.66);
    for &(x, y) in spots.iter().take([0, 6, 14][s.freckles.min(2) as usize]) {
        p.fill(&circle(x, y, 3.6), fc);
    }

    // ---- hair on top
    match s.texture {
        Texture::Straight if s.long_hair => {
            // middle part, smooth curtains past the jaw with a blunt cut
            let cap = [
                M(330.0, 66.0), Q(262.0, 100.0, 234.0, 152.0), Q(194.0, 200.0, 180.0, 236.0), L(176.0, 470.0), L(134.0, 472.0), L(132.0, 236.0),
                C(124.0, 110.0, 216.0, 30.0, 330.0, 30.0), C(444.0, 30.0, 536.0, 110.0, 528.0, 236.0),
                L(526.0, 472.0), L(484.0, 470.0), L(480.0, 236.0), Q(466.0, 200.0, 426.0, 152.0), Q(398.0, 100.0, 330.0, 66.0), Z,
            ];
            p.fill(&cap, hair);
            p.stroke(&cap, INK, LINE);
            p.stroke(&[M(318.0, 52.0), Q(240.0, 80.0, 206.0, 170.0)], hair_line, 2.5);
            p.stroke(&[M(342.0, 52.0), Q(420.0, 80.0, 454.0, 170.0)], hair_line, 2.5);
            p.stroke(&[M(156.0, 200.0), L(155.0, 460.0)], hair_line, 2.5);
            p.stroke(&[M(504.0, 200.0), L(505.0, 460.0)], hair_line, 2.5);
        }
        Texture::Wavy if s.long_hair => {
            // middle part, curtains that wave down past the jaw and flick out
            let cap = [
                M(330.0, 66.0), Q(262.0, 100.0, 234.0, 152.0), Q(190.0, 196.0, 180.0, 236.0), Q(166.0, 282.0, 178.0, 322.0), Q(196.0, 360.0, 178.0, 394.0), Q(162.0, 430.0, 180.0, 462.0),
                Q(158.0, 478.0, 134.0, 462.0), Q(118.0, 428.0, 132.0, 394.0), Q(144.0, 360.0, 128.0, 322.0), Q(116.0, 282.0, 130.0, 238.0), Q(150.0, 194.0, 140.0, 150.0),
                C(156.0, 68.0, 236.0, 30.0, 330.0, 30.0), C(424.0, 30.0, 504.0, 68.0, 520.0, 150.0),
                Q(510.0, 194.0, 530.0, 238.0), Q(544.0, 282.0, 532.0, 322.0), Q(516.0, 360.0, 528.0, 394.0), Q(542.0, 428.0, 526.0, 462.0), Q(502.0, 478.0, 480.0, 462.0), Q(498.0, 430.0, 482.0, 394.0),
                Q(464.0, 360.0, 482.0, 322.0), Q(494.0, 282.0, 480.0, 236.0), Q(470.0, 196.0, 426.0, 152.0), Q(398.0, 100.0, 330.0, 66.0), Z,
            ];
            p.fill(&cap, hair);
            p.stroke(&cap, INK, LINE);
            p.stroke(&[M(318.0, 52.0), Q(256.0, 70.0, 238.0, 100.0), Q(222.0, 130.0, 200.0, 160.0)], hair_line, 2.5);
            p.stroke(&[M(342.0, 52.0), Q(404.0, 70.0, 422.0, 100.0), Q(438.0, 130.0, 460.0, 160.0)], hair_line, 2.5);
            p.stroke(&[M(150.0, 200.0), Q(140.0, 240.0, 154.0, 280.0), Q(168.0, 320.0, 152.0, 360.0), Q(138.0, 400.0, 156.0, 440.0)], hair_line, 2.5);
            p.stroke(&[M(510.0, 200.0), Q(520.0, 240.0, 506.0, 280.0), Q(492.0, 320.0, 508.0, 360.0), Q(522.0, 400.0, 504.0, 440.0)], hair_line, 2.5);
        }
        Texture::Straight => {
            // short side-swept cut: part on the left, fringe sweeping right to a point
            let cap = [
                M(152.0, 232.0), C(132.0, 140.0, 200.0, 52.0, 318.0, 48.0), C(440.0, 46.0, 528.0, 120.0, 516.0, 226.0),
                L(500.0, 232.0), Q(486.0, 170.0, 420.0, 152.0), Q(330.0, 128.0, 252.0, 112.0), Q(214.0, 150.0, 172.0, 232.0), Z,
            ];
            p.fill(&cap, hair);
            p.stroke(&cap, INK, LINE);
            p.stroke(&[M(262.0, 70.0), Q(370.0, 82.0, 440.0, 150.0)], hair_line, 2.5);
            p.stroke(&[M(318.0, 60.0), Q(430.0, 76.0, 492.0, 196.0)], hair_line, 2.5);
            p.stroke(&[M(242.0, 86.0), Q(320.0, 98.0, 372.0, 134.0)], hair_line, 2.5);
            p.stroke(&[M(236.0, 96.0), Q(196.0, 130.0, 176.0, 196.0)], hair_line, 2.5);
        }
        Texture::Wavy => {
            // short tousled cut: lumpy top, two-wave fringe
            let cap = [
                M(150.0, 234.0), C(128.0, 170.0, 150.0, 96.0, 214.0, 70.0), Q(240.0, 40.0, 282.0, 56.0), Q(316.0, 30.0, 358.0, 52.0),
                Q(400.0, 34.0, 438.0, 64.0), C(500.0, 90.0, 530.0, 160.0, 514.0, 232.0), Q(500.0, 214.0, 494.0, 196.0),
                Q(478.0, 142.0, 432.0, 164.0), Q(392.0, 184.0, 370.0, 146.0), Q(346.0, 112.0, 300.0, 144.0),
                Q(262.0, 172.0, 240.0, 138.0), Q(214.0, 118.0, 190.0, 160.0), Q(172.0, 196.0, 150.0, 234.0), Z,
            ];
            p.fill(&cap, hair);
            p.stroke(&cap, INK, LINE);
            p.stroke(&[M(250.0, 76.0), Q(276.0, 98.0, 262.0, 116.0), Q(250.0, 132.0, 268.0, 146.0)], hair_line, 2.5);
            p.stroke(&[M(334.0, 64.0), Q(360.0, 88.0, 344.0, 106.0), Q(330.0, 122.0, 348.0, 132.0)], hair_line, 2.5);
            p.stroke(&[M(414.0, 74.0), Q(440.0, 100.0, 424.0, 122.0), Q(410.0, 140.0, 430.0, 150.0)], hair_line, 2.5);
        }
        Texture::Curly => {
            // base fill so no skin peeks between curls, then curls from back to front
            let base = [M(150.0, 240.0), C(140.0, 120.0, 225.0, 54.0, 330.0, 52.0), C(435.0, 54.0, 520.0, 120.0, 510.0, 240.0), Q(470.0, 170.0, 330.0, 168.0), Q(190.0, 170.0, 150.0, 240.0), Z];
            p.fill(&base, hair);
            let mut curls: Vec<(f64, f64, f64)> = vec![];
            if s.long_hair {
                // curls tumbling down both sides, over the shoulders
                for i in (0..5).rev() {
                    let y = 280.0 + 46.0 * i as f64;
                    let dx = if i % 2 == 0 { 0.0 } else { 8.0 };
                    curls.push((142.0 + dx, y, 36.0));
                    curls.push((518.0 - dx, y, 36.0));
                }
            }
            for i in 0..11 {
                let a = (-190.0 + 200.0 * i as f64 / 10.0f64).to_radians();
                curls.push((hx + 178.0 * a.cos(), 218.0 + 158.0 * a.sin(), 40.0));
            }
            for i in 0..6 {
                let x = 220.0 + 44.0 * i as f64;
                curls.push((x, 168.0 + if i % 2 == 0 { 0.0 } else { 8.0 }, 34.0));
            }
            for &(x, y, r) in &curls {
                p.fill(&circle(x, y, r), hair);
                p.stroke(&arc(x, y, r, r, 0.0, 360.0, 0.03), INK, LINE * 0.85);
                p.stroke(&arc(x + 2.0, y + 2.0, r * 0.45, r * 0.45, 200.0, 470.0, 0.0), hair_line, 2.5);
            }
        }
    }

    // ---- eyes
    for &(eye, ix, iy, ir) in &[
        ([M(183.0, 258.0), Q(236.0, 204.0, 294.0, 252.0), Q(246.0, 302.0, 183.0, 258.0), Z], 245.0, 254.0, 25.0),
        ([M(354.0, 252.0), Q(410.0, 198.0, 471.0, 245.0), Q(416.0, 296.0, 354.0, 252.0), Z], 414.0, 248.0, 24.0),
    ] {
        p.fill(&eye, WHITE);
        p.fill(&circle(ix, iy, ir), rgb(s.eyes, 1.0));
        p.stroke(&arc(ix, iy, ir, ir, 0.0, 360.0, 0.0), rgb(s.eyes, 0.55), 2.0);
        p.fill(&circle(ix + 4.0, iy - 5.0, 8.0), INK);
        p.fill(&circle(ix - 11.0, iy - 10.0, 4.5), WHITE);
        p.stroke(&eye, INK, LINE * 0.8);
    }

    // ---- eyebrows: tapered (thick inner end), ink outline like the hair; colour = hair colour
    let brows = [
        [M(196.0, 218.0), Q(232.0, 186.0, 284.0, 192.0), Q(296.0, 194.0, 294.0, 204.0), Q(292.0, 212.0, 282.0, 210.0), Q(238.0, 202.0, 202.0, 222.0), Q(194.0, 224.0, 196.0, 218.0), Z],
        [M(368.0, 192.0), Q(420.0, 180.0, 466.0, 210.0), Q(470.0, 216.0, 462.0, 218.0), Q(424.0, 196.0, 374.0, 208.0), Q(362.0, 210.0, 362.0, 202.0), Q(362.0, 194.0, 368.0, 192.0), Z],
    ];
    for b in &brows {
        p.fill(b, hair);
        p.stroke(b, INK, LINE * 0.8);
    }

    // ---- smug half-smile
    p.stroke(&[M(280.0, 351.0), Q(286.0, 362.0, 302.0, 362.0), Q(366.0, 362.0, 409.0, 336.0)], INK, LINE * 0.9);
    p.stroke(&[M(404.0, 341.0), L(412.0, 330.0)], INK, LINE * 0.9);
}

// ------------------------------------------------------------------ SVG backend
pub struct Svg(pub String);

fn path_d(path: &[Seg]) -> String {
    let mut d = String::new();
    for s in path {
        let t = match *s {
            M(x, y) => format!("M{x:.1} {y:.1}"),
            L(x, y) => format!("L{x:.1} {y:.1}"),
            Q(a, b, x, y) => format!("Q{a:.1} {b:.1} {x:.1} {y:.1}"),
            C(a, b, c, e, x, y) => format!("C{a:.1} {b:.1} {c:.1} {e:.1} {x:.1} {y:.1}"),
            Z => "Z".into(),
        };
        d.push_str(&t);
    }
    d
}
fn hex(c: Rgb) -> String {
    format!("#{:02x}{:02x}{:02x}", (c.0 * 255.0).round() as u8, (c.1 * 255.0).round() as u8, (c.2 * 255.0).round() as u8)
}
impl Pen for Svg {
    fn fill(&mut self, path: &[Seg], c: Rgb) {
        self.0 += &format!("<path d=\"{}\" fill=\"{}\"/>\n", path_d(path), hex(c));
    }
    fn stroke(&mut self, path: &[Seg], c: Rgb, w: f64) {
        self.0 += &format!("<path d=\"{}\" fill=\"none\" stroke=\"{}\" stroke-width=\"{w:.1}\" stroke-linecap=\"round\" stroke-linejoin=\"round\"/>\n", path_d(path), hex(c));
    }
}

pub fn to_svg(s: &Spec) -> String {
    let mut pen = Svg(String::new());
    draw(&mut pen, s);
    let (x, y, w, h) = VIEW;
    format!("<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"{x} {y} {w} {h}\" width=\"{}\" height=\"{}\">\n{}</svg>\n", w / 2.0, h / 2.0, pen.0)
}
