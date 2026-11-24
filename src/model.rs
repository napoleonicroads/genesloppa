//! G25 admixture modeling, ported from Vahaduo's "Single" mode (admixture.js): the slot-based
//! fast Monte Carlo fit and its deterministic second stage, the optional "add distance column"
//! (ADC) penalty, the R#P limit on the number of populations, and grouping samples by
//! population name (the part before ':').

use crate::linalg::Rng;
use std::collections::BTreeMap;

pub struct Sheet {
    pub labels: Vec<String>,
    pub rows: Vec<Vec<f64>>,
}

/// Parse G25 lines ("Label,c1,c2,..."); every row must have the same number of dimensions.
pub fn parse_sheet(text: &str) -> Result<Sheet, String> {
    let mut s = Sheet { labels: vec![], rows: vec![] };
    for (n, line) in text.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let mut it = line.split(',');
        let label = it.next().unwrap_or("").trim().to_string();
        let nums: Result<Vec<f64>, _> = it.map(|x| x.trim().parse::<f64>()).collect();
        let Ok(nums) = nums else {
            if n == 0 {
                continue; // header row
            }
            return Err(format!("line {}: not a number in “{}”", n + 1, short(line)));
        };
        if nums.is_empty() {
            continue;
        }
        if let Some(first) = s.rows.first() {
            if first.len() != nums.len() {
                return Err(format!("line {}: has {} values, expected {}", n + 1, nums.len(), first.len()));
            }
        }
        s.labels.push(label);
        s.rows.push(nums);
    }
    if s.rows.is_empty() {
        return Err("no coordinates found".into());
    }
    Ok(s)
}

fn short(s: &str) -> String {
    s.chars().take(40).collect()
}

#[derive(Clone, Copy)]
pub struct Options {
    pub cycles: f64,     // Vahaduo "cycles multiplier" (default 1)
    pub adc: f64,        // add-distance-column multiplier, 0 = off (Vahaduo shows e.g. 0.25x)
    pub recalc: bool,    // ADC "RC": re-optimize without the extra column at the end
    pub npop: usize,     // R#P: restrict to N populations, 0 = use all
    pub npop_max: usize, // with npop > 0: also try npop+1 ..= npop_max and keep the best (see `fit`)
    pub aggregate: bool, // sum samples of the same population
    pub zeroes: bool,    // print 0% rows
}
impl Default for Options {
    fn default() -> Self {
        Options { cycles: 1.0, adc: 0.0, recalc: false, npop: 0, npop_max: 0, aggregate: true, zeroes: false }
    }
}

pub struct Fit {
    pub distance: f64,
    pub rows: Vec<(String, f64)>, // name, fraction (0..1), sorted descending
    pub sources: usize,
    pub cycles_or_iters: usize,
    pub pops: usize,
    /// populations the model was restricted to (R#P), 0 = all
    pub npop: usize,
}

/// A bigger R#P model is only preferred when it beats the smaller one by more than this share.
const NPOP_TOLERANCE: f64 = 0.05;

const SLOTS: usize = 500;

// ------------------------------------------------------------------ core: fastMonteCarlo
struct FmcResult {
    distance: f64,
    scores: Vec<f64>,
}

fn sqdist(p: &[f64]) -> f64 {
    p.iter().map(|x| x * x).sum()
}

/// `source` rows and `target` are already divided by the base slot count (prepareSource/prepareTarget).
/// Sources are given as row indices into `all`, so callers never copy rows.
fn fast_monte_carlo(target: &[f64], all: &[Vec<f64>], rows: &[usize], slots: usize, cycles_mult: f64, adc: f64, recalc: bool, rng: &mut Rng) -> FmcResult {
    let n = rows.len();
    let base = target.len();
    let dc = adc; // Vahaduo divides its UI value by 8; `adc` here is already the displayed multiplier
    let stride = if dc > 0.0 { base + 1 } else { base };
    let mut dim = stride;
    let cycles = (n as f64 * cycles_mult / 4.0).ceil() as usize;
    let mut scores = vec![0.0; n];
    // shift sources by the target into one contiguous matrix; optional distance column
    let mut src = vec![0.0; n * stride];
    for (r, &i) in rows.iter().enumerate() {
        let s = &mut src[r * stride..r * stride + base];
        for ((o, a), b) in s.iter_mut().zip(&all[i]).zip(target) {
            *o = a - b;
        }
        if dc > 0.0 {
            let d = sqdist(s).sqrt();
            src[r * stride + base] = dc * d;
        }
    }
    let row = |i: usize, dim: usize| -> std::ops::Range<usize> { i * stride..i * stride + dim };
    let rand_idx = |rng: &mut Rng, avoid: usize| -> usize {
        loop {
            let r = (rng.next_u64() % n as u64) as usize;
            if r != avoid || n == 1 {
                return r;
            }
        }
    };
    let build = |slots_v: &[usize], dim: usize, src: &[f64]| -> Vec<f64> {
        let mut p = vec![0.0; dim];
        for &i in slots_v {
            for (a, b) in p.iter_mut().zip(&src[row(i, dim)]) {
                *a += b;
            }
        }
        p
    };

    if n == 1 {
        let p = build(&vec![0; slots], dim, &src);
        let p = if dc > 0.0 { &p[..dim - 1] } else { &p[..] };
        return FmcResult { distance: round8(sqdist(p).sqrt()), scores: vec![1.0] };
    }

    let mut cur_slots: Vec<usize> = (0..slots).map(|_| rand_idx(rng, usize::MAX)).collect();
    let mut cur = build(&cur_slots, dim, &src);
    let mut cur_d = sqdist(&cur);
    let mut next = vec![0.0; dim];
    let zero: f64 = std::iter::empty::<f64>().sum(); // `sqdist`'s starting value
    for _ in 0..cycles {
        // Draw each slot's candidate right before testing it. That uses the RNG in the same
        // order as drawing all 500 up front (slot j's draw only depends on cur_slots[j]), and a
        // partial distance that already reaches cur_d can only grow, so we can stop early.
        for j in 0..slots {
            let nj = rand_idx(rng, cur_slots[j]);
            let (a, b) = (&src[row(cur_slots[j], dim)], &src[row(nj, dim)]);
            let mut nd = zero;
            let mut full = true;
            for k in 0..dim {
                let x = cur[k] - a[k] + b[k];
                next[k] = x;
                nd = nd + x * x;
                if nd >= cur_d {
                    full = false;
                    break;
                }
            }
            if full && nd < cur_d {
                cur_slots[j] = nj;
                cur.copy_from_slice(&next);
                cur_d = nd;
            }
        }
    }
    let mut counts = vec![0i64; n];
    for &i in &cur_slots {
        counts[i] += 1;
    }

    // second stage: deterministic one-slot moves between used sources until no improvement
    let second_stage = |counts: &mut Vec<i64>, cur: &mut Vec<f64>, src: &[f64], dim: usize| {
        let mut ranking: Vec<(usize, i64)> = counts.iter().enumerate().filter(|(_, &c)| c > 0).map(|(i, &c)| (i, c)).collect();
        ranking.sort_by(|a, b| b.1.cmp(&a.1));
        let big = |d: f64| (d * 1e17).round();
        let mut cd = big(sqdist(&cur[..dim]));
        let mut nxt = vec![0.0; dim];
        loop {
            let prev = cd;
            for i in (0..ranking.len()).rev() {
                if ranking[i].1 > 0 {
                    for j in 0..ranking.len() {
                        if i == j {
                            continue;
                        }
                        let (a, b) = (&src[row(ranking[i].0, dim)], &src[row(ranking[j].0, dim)]);
                        for k in 0..dim {
                            nxt[k] = cur[k] - a[k] + b[k];
                        }
                        let nd = big(sqdist(&nxt));
                        if nd < cd {
                            ranking[i].1 -= 1;
                            ranking[j].1 += 1;
                            cur[..dim].copy_from_slice(&nxt);
                            cd = nd;
                            break;
                        }
                    }
                }
            }
            if cd >= prev {
                break;
            }
        }
        for &(i, c) in &ranking {
            counts[i] = c;
        }
    };
    second_stage(&mut counts, &mut cur, &src, dim);

    if dc > 0.0 && recalc {
        dim -= 1; // rows keep their stride; only the first `dim` values are read from now on
        cur.pop();
        second_stage(&mut counts, &mut cur, &src, dim);
    }
    for i in 0..n {
        scores[i] = counts[i] as f64 / slots as f64;
    }
    let final_point = if dc > 0.0 && !recalc { &cur[..dim - 1] } else { &cur[..dim] };
    FmcResult { distance: round8(sqdist(final_point).sqrt()), scores }
}

fn round8(x: f64) -> f64 {
    (x * 1e8).round() / 1e8
}

fn pop_name(label: &str) -> &str {
    label.split(':').next().unwrap_or(label)
}

fn aggregate(rows: Vec<(String, f64)>) -> Vec<(String, f64)> {
    let mut m: BTreeMap<String, f64> = BTreeMap::new();
    for (name, v) in rows {
        *m.entry(pop_name(&name).to_string()).or_insert(0.0) += v;
    }
    m.into_iter().collect()
}

// ------------------------------------------------------------------ R#P (nPops)
fn n_pops(target: &[f64], labels: &[String], source: &[Vec<f64>], base_slots: usize, o: &Options, rng: &mut Rng) -> (FmcResult, Vec<String>, usize, usize) {
    // group samples by population name
    let mut groups: BTreeMap<&str, Vec<usize>> = BTreeMap::new();
    for (i, l) in labels.iter().enumerate() {
        groups.entry(pop_name(l)).or_default().push(i);
    }
    let mut pops: Vec<Vec<usize>> = groups.into_values().collect();
    let mut counter = 0usize;

    let run = |set: &[usize], pops: &[Vec<usize>], slots: usize, cm: f64, adc: f64, rc: bool, rng: &mut Rng, counter: &mut usize| -> FmcResult {
        *counter += 1;
        let cur: Vec<usize> = set.iter().flat_map(|&p| pops[p].iter().copied()).collect();
        fast_monte_carlo(target, source, &cur, slots, cm, adc, rc, rng)
    };

    // screening: six quick fits over all populations, keep those scoring > 2% in total
    let all: Vec<usize> = (0..pops.len()).collect();
    let id_of: Vec<usize> = pops.iter().enumerate().flat_map(|(p, v)| std::iter::repeat(p).take(v.len())).collect();
    let screens = [(50, true, 0.5, 5.0), (50, false, 0.0, 5.0), (50, true, 1.0, 5.0), (50, false, 0.0, 5.0), (50, true, 2.0, 5.0), (500, false, 0.0, 2.0)];
    let mut tally = vec![0.0; pops.len()];
    for &(sl, adc_on, adcm, cm) in &screens {
        // Vahaduo passes (adc, adcmultiplier) with adc=true meaning "recalculate"; multiplier/8 inside
        let r = run(&all, &pops, sl, cm, adcm / 8.0, adc_on, rng, &mut counter);
        for (k, s) in r.scores.iter().enumerate() {
            tally[id_of[k]] += s;
        }
    }
    let mut order: Vec<usize> = (0..pops.len()).collect();
    order.sort_by(|&a, &b| tally[b].partial_cmp(&tally[a]).unwrap_or(std::cmp::Ordering::Equal));
    let kept: Vec<usize> = order.into_iter().take_while(|&p| tally[p] > 0.02).collect();
    pops = kept.iter().map(|&p| pops[p].clone()).collect();
    let pop_num = pops.len();

    let names = |set: &[usize], pops: &[Vec<usize>]| -> Vec<String> { set.iter().flat_map(|&p| pops[p].iter().map(|&i| labels[i].clone())).collect() };

    let best_set: Vec<usize> = if pop_num <= o.npop {
        (0..pop_num).collect()
    } else {
        let start: Vec<usize> = (0..o.npop).collect();
        let mut runs: Vec<(f64, Vec<usize>)> = vec![];
        for _ in 0..(30 + pop_num) {
            let mut set = start.clone();
            let mut best = run(&set, &pops, 35, o.cycles, o.adc, o.recalc, rng, &mut counter);
            for _ in 0..pop_num {
                for j in 0..o.npop {
                    let mut nxt = set.clone();
                    let mut np = (rng.next_u64() % pop_num as u64) as usize;
                    while np == nxt[j] || set.contains(&np) {
                        np = (rng.next_u64() % pop_num as u64) as usize;
                    }
                    nxt[j] = np;
                    let r = run(&nxt, &pops, 35, o.cycles, o.adc, o.recalc, rng, &mut counter);
                    if r.distance < best.distance {
                        best = r;
                        set = nxt;
                    }
                }
            }
            runs.push((best.distance, set));
        }
        runs.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
        runs.swap_remove(0).1
    };
    let fin = run(&best_set, &pops, base_slots, o.cycles, o.adc, o.recalc, rng, &mut counter);
    (fin, names(&best_set, &pops), pop_num, counter)
}

// ------------------------------------------------------------------ public entry (singleFMC)
/// With a range (npop..=npop_max), fit each size and keep the fewest populations whose distance
/// is within 5% of the largest model's: more populations almost never fit worse, so the extra
/// ones have to earn their place.
pub fn fit(target: &[f64], sources: &Sheet, o: &Options) -> Result<Fit, String> {
    if o.npop == 0 || o.npop_max <= o.npop {
        return fit_once(target, sources, o);
    }
    let mut fits = vec![];
    for n in o.npop..=o.npop_max {
        fits.push(fit_once(target, sources, &Options { npop: n, npop_max: 0, ..*o })?);
    }
    let best = fits.iter().map(|f| f.distance).fold(f64::INFINITY, f64::min);
    let limit = best * (1.0 + NPOP_TOLERANCE);
    let i = fits.iter().position(|f| f.distance <= limit).unwrap_or(fits.len() - 1);
    let mut chosen = fits.swap_remove(i);
    chosen.cycles_or_iters = fits.iter().map(|f| f.cycles_or_iters).sum::<usize>() + chosen.cycles_or_iters;
    Ok(chosen)
}

/// Seed from the inputs (FNV-1a), so the same target, sources and settings always give the
/// same fit. Vahaduo seeds from the clock, which makes results change from run to run.
fn input_seed(target: &[f64], sources: &Sheet, o: &Options) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    let mut eat = |bytes: &[u8]| {
        for &b in bytes {
            h = (h ^ b as u64).wrapping_mul(0x0100_0000_01b3);
        }
    };
    for x in target {
        eat(&x.to_bits().to_le_bytes());
    }
    for (label, row) in sources.labels.iter().zip(&sources.rows) {
        eat(label.as_bytes());
        for x in row {
            eat(&x.to_bits().to_le_bytes());
        }
    }
    eat(&[o.npop as u8, o.recalc as u8]);
    eat(&o.cycles.to_bits().to_le_bytes());
    eat(&o.adc.to_bits().to_le_bytes());
    h | 1
}

fn fit_once(target: &[f64], sources: &Sheet, o: &Options) -> Result<Fit, String> {
    if target.len() != sources.rows[0].len() {
        return Err(format!("target has {} values but sources have {}", target.len(), sources.rows[0].len()));
    }
    let mut rng = Rng(input_seed(target, sources, o));
    let t: Vec<f64> = target.iter().map(|x| x / SLOTS as f64).collect();
    let s: Vec<Vec<f64>> = sources.rows.iter().map(|r| r.iter().map(|x| x / SLOTS as f64).collect()).collect();

    let (res, names, pops, iters) = if o.npop == 0 {
        let all: Vec<usize> = (0..s.len()).collect();
        let r = fast_monte_carlo(&t, &s, &all, SLOTS, o.cycles, o.adc, o.recalc, &mut rng);
        let cyc = (s.len() as f64 * o.cycles / 4.0).ceil() as usize;
        (r, sources.labels.clone(), s.len(), cyc)
    } else {
        n_pops(&t, &sources.labels, &s, SLOTS, o, &mut rng)
    };
    let mut rows: Vec<(String, f64)> = names.into_iter().zip(res.scores).collect();
    if o.aggregate {
        rows = aggregate(rows);
    }
    if !o.zeroes {
        rows.retain(|r| r.1 != 0.0);
    }
    rows.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    Ok(Fit { distance: res.distance, rows, sources: s.len(), cycles_or_iters: iters, pops, npop: o.npop })
}

impl Fit {
    /// Vahaduo-style text output
    pub fn render(&self, target_name: &str, o: &Options) -> String {
        let mut s = format!("Target: {target_name}\nDistance: {:.4}% / {:.8}", 100.0 * self.distance, self.distance);
        if self.npop > 0 {
            s.push_str(&format!(" | R{}P", self.npop));
        }
        if o.adc > 0.0 {
            s.push_str(&format!(" | ADC: {}x{}", o.adc, if o.recalc { " RC" } else { "" }));
        }
        s.push('\n');
        for (name, v) in &self.rows {
            s.push_str(&format!("{:>6.1}  {name}\n", v * 100.0));
        }
        s
    }
}

// ---------------------------------------------------------------- distance (Vahaduo's "Distance" tab)

/// Euclidean distance from `target` to every source row, closest first.
pub fn distances(target: &[f64], sources: &Sheet) -> Result<Vec<(String, f64)>, String> {
    let d = target.len();
    if let Some(r) = sources.rows.first() {
        if r.len() != d {
            return Err(format!("the target has {d} coordinates but the sources have {}", r.len()));
        }
    }
    let mut out: Vec<(String, f64)> = sources
        .labels
        .iter()
        .zip(&sources.rows)
        .map(|(l, r)| (l.clone(), r.iter().zip(target).map(|(a, b)| (a - b) * (a - b)).sum::<f64>().sqrt()))
        .collect();
    out.sort_by(|a, b| a.1.total_cmp(&b.1).then_with(|| a.0.cmp(&b.0)));
    Ok(out)
}

/// The same text Vahaduo's Distance tab produces, ready to paste.
pub fn render_distances(target_name: &str, rows: &[(String, f64)]) -> String {
    let mut s = format!("Distance to:\t{target_name}\n");
    for (name, v) in rows {
        s.push_str(&format!("{v:.8}\t{name}\n"));
    }
    s
}

/// Cell colour for position `t` (0 = closest shown, 1 = furthest shown) on the green -> yellow -> red
/// scale: returns (background, text) as RGB in 0..1.
pub fn distance_colors(t: f64) -> ([f64; 3], [f64; 3]) {
    let t = t.clamp(0.0, 1.0);
    let h = 130.0 - 130.0 * t;
    let bump = (std::f64::consts::PI * t).sin();
    (hsl(h, 0.68 + 0.20 * bump, 0.60 - 0.02 * bump), hsl(h, 0.75, 0.14))
}

fn hsl(h: f64, s: f64, l: f64) -> [f64; 3] {
    let c = (1.0 - (2.0 * l - 1.0).abs()) * s;
    let hp = h / 60.0;
    let x = c * (1.0 - (hp % 2.0 - 1.0).abs());
    let (r, g, b) = match hp as u32 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    let m = l - c / 2.0;
    [r + m, g + m, b + m]
}

// ---------------------------------------------------------------- distance gradients (Vahaduo-style)

/// Steepness slider range: at 0 the colours go from green to full red over a 0.30 distance
/// gap (gentle), at 1 over a 0.01 gap (steep). Log-spaced so the middle of the slider is useful.
pub const SPAN_GENTLE: f64 = 0.30;
pub const SPAN_STEEP: f64 = 0.01;

/// Distance gap (from the closest row) that maps to full red, for slider position `s` in 0..=1.
pub fn span_for_steepness(s: f64) -> f64 {
    SPAN_GENTLE * (SPAN_STEEP / SPAN_GENTLE).powf(s.clamp(0.0, 1.0))
}
/// Inverse of `span_for_steepness`, clamped to the slider range.
pub fn steepness_for_span(span: f64) -> f64 {
    if span <= 0.0 { return 1.0 }
    ((span / SPAN_GENTLE).ln() / (SPAN_STEEP / SPAN_GENTLE).ln()).clamp(0.0, 1.0)
}
/// (closest distance, gap to full red) for the rows shown. Auto (`manual` = None) stretches
/// the gradient from the closest to the furthest row, so fewer rows = steeper.
pub fn gradient_span(rows: &[(String, f64)], manual: Option<f64>) -> (f64, f64) {
    let lo = rows.first().map_or(0.0, |r| r.1);
    let span = manual.unwrap_or_else(|| rows.last().map_or(0.0, |r| r.1) - lo);
    (lo, span)
}
