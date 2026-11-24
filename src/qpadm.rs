//! qpAdm / qpWave in pure Rust, run straight on the reference pack's genotypes.
//!
//! Same model as ADMIXTOOLS / ADMIXTOOLS 2:
//! 1. Allele counts per population and SNP (pseudohaploid ancient samples count one allele).
//! 2. f2 for every pair of populations, averaged in genome blocks (5 Mb by default), with the
//!    small-sample heterozygosity correction. The blocks drive the jackknife.
//! 3. X = f4(Target, Source_i; Right_0, Right_j) for each source i and right j >= 1.
//!    If Target = sum(w_i * Source_i) then sum(w_i * X_i) = 0, so the weights are the left null
//!    vector of X, scaled to sum to 1. We minimise chisq = r' Q^-1 r where r = X'w and Q is the
//!    jackknife covariance of r. Degrees of freedom: (rights - 1) - (sources - 1), as in qpAdm.
//! 4. Standard errors of the weights come from the same jackknife.
//!
//! Counts per population are cached in memory, so later runs are quick.

use crate::pack::Pack;
use crate::raw::RawGenotypes;
use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

/// Name used for the user's own DNA as a qpAdm population.
pub const MY_DNA: &str = "My DNA";

#[derive(Clone, Copy, Debug)]
pub struct Settings {
    /// genome block size for the jackknife, in megabases
    pub block_mb: f64,
    /// false: only SNPs every population in the model has (ADMIXTOOLS 2 default);
    /// true: each pair uses every SNP both of them have ("allsnps")
    pub allsnps: bool,
    /// added to the covariance diagonal (relative), keeps it invertible
    pub fudge: f64,
    pub p_threshold: f64,
}
impl Default for Settings {
    fn default() -> Self {
        Settings { block_mb: 5.0, allsnps: false, fudge: 1e-4, p_threshold: 0.05 }
    }
}

/// Allele counts of one population at every pack SNP.
pub struct Counts {
    pub name: String,
    pub samples: usize,
    /// copies of the pack's allele 1
    pub alt: Vec<u16>,
    /// alleles observed (0 = missing)
    pub tot: Vec<u16>,
}
impl Counts {
    pub fn snps_covered(&self) -> usize {
        self.tot.iter().filter(|&&t| t > 0).count()
    }
}

fn cache() -> &'static Mutex<HashMap<String, Arc<Counts>>> {
    static C: OnceLock<Mutex<HashMap<String, Arc<Counts>>>> = OnceLock::new();
    C.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Populations that have genotypes in the pack: (name, samples), sorted by name.
pub fn populations(pack: &Pack) -> Vec<(String, usize)> {
    let mut n = vec![0usize; pack.pops.len()];
    for r in &pack.refs {
        n[r.pop as usize] += 1;
    }
    let mut v: Vec<(String, usize)> = pack.pops.iter().zip(n).filter(|(_, c)| *c > 0).map(|(p, c)| (p.name.clone(), c)).collect();
    v.sort_by(|a, b| a.0.to_lowercase().cmp(&b.0.to_lowercase()));
    v
}

/// Allele counts for these populations (cached). `user` supplies "My DNA".
pub fn counts(pack: &Pack, names: &[String], user: Option<&RawGenotypes>) -> Result<Vec<Arc<Counts>>, String> {
    let missing: Vec<String> = {
        let c = cache().lock().unwrap();
        names.iter().filter(|n| !c.contains_key(*n)).cloned().collect()
    };
    if !missing.is_empty() {
        let ns = pack.snps.len();
        let mut want: HashMap<u32, usize> = HashMap::new(); // pack pop index -> slot
        let mut built: Vec<Counts> = vec![];
        for name in &missing {
            if name == MY_DNA {
                let g = user.ok_or("convert or load your DNA file first to use it in qpAdm")?;
                built.push(user_counts(pack, g));
                continue;
            }
            let pi = pack.pops.iter().position(|p| &p.name == name).ok_or(format!("{name} isn't in the reference pack"))?;
            want.insert(pi as u32, built.len());
            built.push(Counts { name: name.clone(), samples: 0, alt: vec![0; ns], tot: vec![0; ns] });
        }
        let rows: Vec<usize> = (0..pack.refs.len()).filter(|&i| want.contains_key(&pack.refs[i].pop)).collect();
        for (i, row) in rows.iter().zip(pack.sample_rows(&rows)?) {
            let slot = want[&pack.refs[*i].pop];
            let c = &mut built[slot];
            c.samples += 1;
            // pseudohaploid (most ancient samples): no heterozygous calls -> one allele per site
            let (mut het, mut obs) = (0usize, 0usize);
            for j in 0..ns {
                let g = (row[j >> 2] >> ((j & 3) * 2)) & 3;
                if g != 3 {
                    obs += 1;
                    het += (g == 1) as usize;
                }
            }
            let haploid = obs > 0 && (het as f64) < 0.002 * obs as f64;
            for j in 0..ns {
                let g = (row[j >> 2] >> ((j & 3) * 2)) & 3;
                if g == 3 {
                    continue;
                }
                if haploid {
                    c.alt[j] += (g == 2) as u16;
                    c.tot[j] += 1;
                } else {
                    c.alt[j] += g as u16;
                    c.tot[j] += 2;
                }
            }
        }
        let mut cc = cache().lock().unwrap();
        for c in built {
            cc.insert(c.name.clone(), Arc::new(c));
        }
    }
    let c = cache().lock().unwrap();
    Ok(names.iter().map(|n| c[n].clone()).collect())
}

/// The user's raw genotypes as a diploid one-sample population, aligned to the pack's alleles.
fn user_counts(pack: &Pack, g: &RawGenotypes) -> Counts {
    raw_counts(pack, g, MY_DNA)
}

/// One raw DNA file as a diploid one-sample population, aligned to the pack's alleles.
fn raw_counts(pack: &Pack, g: &RawGenotypes, name: &str) -> Counts {
    let comp = |b: u8| match b {
        b'A' => b'T',
        b'T' => b'A',
        b'C' => b'G',
        b'G' => b'C',
        _ => 0,
    };
    let ns = pack.snps.len();
    let mut c = Counts { name: name.into(), samples: 1, alt: vec![0; ns], tot: vec![0; ns] };
    for (j, s) in pack.snps.iter().enumerate() {
        let Some(gt) = g.get(&s.rsid) else { continue };
        let ok = |x: u8| x == s.a1 || x == s.a2;
        let gt = if ok(gt[0]) && ok(gt[1]) {
            *gt
        } else {
            let t = [comp(gt[0]), comp(gt[1])];
            if ok(t[0]) && ok(t[1]) { t } else { continue }
        };
        c.alt[j] = (gt[0] == s.a1) as u16 + (gt[1] == s.a1) as u16;
        c.tot[j] = 2;
    }
    c
}

/// Forget cached counts (e.g. after loading a different DNA file).
pub fn forget(name: &str) {
    cache().lock().unwrap().remove(name);
}

// ------------------------------------------------------------------ your own populations
/// A population the user made: from raw DNA files, or by merging populations.
/// Its counts live in the cache for as long as the app is open.
#[derive(Clone, Debug)]
pub struct Custom {
    pub name: String,
    /// "3 DNA files" or "Serbian + Croatian + Bosnian"
    pub what: String,
    pub samples: usize,
    /// G25 position (mean of the files' converted coordinates, or of the merged members)
    pub g25: Option<Vec<f64>>,
}
fn registry() -> &'static Mutex<(usize, Vec<Custom>)> {
    static R: OnceLock<Mutex<(usize, Vec<Custom>)>> = OnceLock::new();
    R.get_or_init(|| Mutex::new((0, vec![])))
}
/// Your own populations, in the order they were added.
pub fn customs() -> Vec<Custom> {
    registry().lock().unwrap().1.clone()
}
/// Goes up every time a population is added or removed.
pub fn customs_version() -> usize {
    registry().lock().unwrap().0
}
pub fn custom(name: &str) -> Option<Custom> {
    registry().lock().unwrap().1.iter().find(|c| c.name == name).cloned()
}
pub fn remove_custom(name: &str) {
    let mut r = registry().lock().unwrap();
    r.1.retain(|c| c.name != name);
    r.0 += 1;
    forget(name);
}
/// A clean, unused name (an existing population of your own with that name gets replaced).
pub fn check_name(pack: &Pack, name: &str) -> Result<String, String> {
    let n = name.trim().replace([',', '\t', '\n'], " ").replace(" · ", " ");
    let n = n.trim().to_string();
    if n.is_empty() {
        return Err("Give the population a name.".into());
    }
    if n == MY_DNA || pack.pops.iter().any(|p| p.name == n) {
        return Err(format!("\"{n}\" is already a population; pick another name."));
    }
    Ok(n)
}
fn register(c: Counts, what: String, g25: Option<Vec<f64>>) -> Custom {
    let cu = Custom { name: c.name.clone(), what, samples: c.samples, g25 };
    cache().lock().unwrap().insert(c.name.clone(), Arc::new(c));
    let mut r = registry().lock().unwrap();
    r.1.retain(|x| x.name != cu.name);
    r.1.push(cu.clone());
    r.0 += 1;
    cu
}
/// A population from one or more raw DNA files (each file is one diploid person).
pub fn add_from_raw(pack: &Pack, name: &str, files: &[RawGenotypes], g25: Option<Vec<f64>>) -> Result<Custom, String> {
    let name = check_name(pack, name)?;
    if files.is_empty() {
        return Err("No DNA files chosen.".into());
    }
    let ns = pack.snps.len();
    let mut c = Counts { name: name.clone(), samples: 0, alt: vec![0; ns], tot: vec![0; ns] };
    for g in files {
        let one = raw_counts(pack, g, &name);
        if one.snps_covered() < 5_000 {
            return Err(format!("One of the files shares only {} SNPs with the reference pack; is it a raw DNA file?", one.snps_covered()));
        }
        add_into(&mut c, &one);
    }
    let what = if files.len() == 1 { "1 DNA file".to_string() } else { format!("{} DNA files", files.len()) };
    Ok(register(c, what, g25))
}
/// A population made by pooling the samples of other populations (pack ones, your own, or My DNA).
pub fn add_merged(pack: &Pack, name: &str, members: &[String], user: Option<&RawGenotypes>, g25: Option<Vec<f64>>) -> Result<Custom, String> {
    let name = check_name(pack, name)?;
    let mut members: Vec<String> = members.iter().map(|m| m.trim().to_string()).filter(|m| !m.is_empty()).collect();
    members.dedup();
    if members.len() < 2 {
        return Err("List at least two populations to merge.".into());
    }
    if members.contains(&name) {
        return Err("A merged population can't contain itself.".into());
    }
    let parts = counts(pack, &members, user)?;
    let ns = pack.snps.len();
    let mut c = Counts { name: name.clone(), samples: 0, alt: vec![0; ns], tot: vec![0; ns] };
    for p in &parts {
        add_into(&mut c, p);
    }
    Ok(register(c, members.join(" + "), g25))
}
fn add_into(c: &mut Counts, p: &Counts) {
    c.samples += p.samples;
    for j in 0..c.alt.len() {
        c.alt[j] = c.alt[j].saturating_add(p.alt[j]);
        c.tot[j] = c.tot[j].saturating_add(p.tot[j]);
    }
}

// ------------------------------------------------------------------ f2 blocks
/// Block-averaged f2 for a set of populations.
pub struct F2 {
    pub names: Vec<String>,
    /// per block, per pair (i<j packed): (sum, snp count)
    blocks: Vec<Vec<(f64, u32)>>,
    /// per pair: totals over all blocks
    totals: Vec<(f64, u64)>,
    pub n_blocks: usize,
    pub n_snps: usize,
}

fn pair(i: usize, j: usize, n: usize) -> usize {
    let (a, b) = if i < j { (i, j) } else { (j, i) };
    a * n - a * (a + 1) / 2 + (b - a - 1)
}

/// Block index of every pack SNP (chromosome + position window).
fn block_ids(pack: &Pack, mb: f64) -> (Vec<u32>, usize) {
    let w = (mb.max(0.5) * 1e6) as u32;
    let mut ids = Vec::with_capacity(pack.snps.len());
    let mut map: HashMap<(u8, u32), u32> = HashMap::new();
    for s in &pack.snps {
        let k = (s.chrom, s.pos / w);
        let n = map.len() as u32;
        ids.push(*map.entry(k).or_insert(n));
    }
    let n = map.len();
    (ids, n)
}

pub fn f2_blocks(pack: &Pack, pops: &[Arc<Counts>], st: &Settings) -> F2 {
    let n = pops.len();
    let np = n * (n - 1) / 2;
    let (bid, nb) = block_ids(pack, st.block_mb);
    let mut blocks = vec![vec![(0.0f64, 0u32); np]; nb];
    let mut used = 0usize;
    let mut p = vec![0.0f64; n];
    let mut h = vec![0.0f64; n];
    let mut ok = vec![false; n];
    for j in 0..pack.snps.len() {
        let mut all = true;
        let mut any = 0;
        for (k, c) in pops.iter().enumerate() {
            let t = c.tot[j];
            ok[k] = t > 0;
            if t > 0 {
                any += 1;
                let pk = c.alt[j] as f64 / t as f64;
                p[k] = pk;
                h[k] = if t > 1 { pk * (1.0 - pk) / (t as f64 - 1.0) } else { 0.0 };
            } else {
                all = false;
            }
        }
        if (!st.allsnps && !all) || any < 2 {
            continue;
        }
        // skip SNPs that are monomorphic across all observed populations
        let (mut lo, mut hi) = (1.0f64, 0.0f64);
        for k in 0..n {
            if ok[k] {
                lo = lo.min(p[k]);
                hi = hi.max(p[k]);
            }
        }
        if hi - lo == 0.0 && (lo == 0.0 || lo == 1.0) {
            continue;
        }
        used += 1;
        let b = &mut blocks[bid[j] as usize];
        for a in 0..n {
            if !ok[a] {
                continue;
            }
            for c in a + 1..n {
                if !ok[c] {
                    continue;
                }
                let d = p[a] - p[c];
                let e = &mut b[pair(a, c, n)];
                e.0 += d * d - h[a] - h[c];
                e.1 += 1;
            }
        }
    }
    // drop empty blocks
    let blocks: Vec<Vec<(f64, u32)>> = blocks.into_iter().filter(|b| b.iter().any(|e| e.1 > 0)).collect();
    let mut totals = vec![(0.0f64, 0u64); np];
    for b in &blocks {
        for (t, e) in totals.iter_mut().zip(b) {
            t.0 += e.0;
            t.1 += e.1 as u64;
        }
    }
    F2 { names: pops.iter().map(|c| c.name.clone()).collect(), n_blocks: blocks.len(), blocks, totals, n_snps: used }
}

impl F2 {
    fn idx(&self, name: &str) -> usize {
        self.names.iter().position(|n| n == name).expect("population in f2 set")
    }
    /// f2 estimate for a pair: all blocks, or all but block `skip`
    fn f2(&self, a: usize, b: usize, skip: Option<usize>) -> f64 {
        if a == b {
            return 0.0;
        }
        let k = pair(a, b, self.names.len());
        let (mut s, mut c) = self.totals[k];
        if let Some(i) = skip {
            s -= self.blocks[i][k].0;
            c -= self.blocks[i][k].1 as u64;
        }
        if c == 0 { f64::NAN } else { s / c as f64 }
    }
    fn f4(&self, a: usize, b: usize, c: usize, d: usize, skip: Option<usize>) -> f64 {
        (self.f2(a, d, skip) + self.f2(b, c, skip) - self.f2(a, c, skip) - self.f2(b, d, skip)) / 2.0
    }
    /// SNPs per block (for jackknife weights), over the pairs of these populations
    fn block_sizes(&self, idx: &[usize]) -> Vec<f64> {
        let n = self.names.len();
        self.blocks
            .iter()
            .map(|bl| {
                let mut m = 0u32;
                for x in 0..idx.len() {
                    for y in x + 1..idx.len() {
                        m = m.max(bl[pair(idx[x], idx[y], n)].1);
                    }
                }
                m as f64
            })
            .collect()
    }
}

// ------------------------------------------------------------------ small linear algebra
fn solve(a: &[Vec<f64>], b: &[f64]) -> Option<Vec<f64>> {
    let n = b.len();
    let mut m: Vec<Vec<f64>> = a.iter().zip(b).map(|(r, &v)| r.iter().copied().chain(std::iter::once(v)).collect()).collect();
    for c in 0..n {
        let piv = (c..n).max_by(|&x, &y| m[x][c].abs().partial_cmp(&m[y][c].abs()).unwrap())?;
        if m[piv][c].abs() < 1e-300 {
            return None;
        }
        m.swap(c, piv);
        for r in 0..n {
            if r != c {
                let f = m[r][c] / m[c][c];
                if f != 0.0 {
                    for k in c..=n {
                        m[r][k] -= f * m[c][k];
                    }
                }
            }
        }
    }
    Some((0..n).map(|i| m[i][n] / m[i][i]).collect())
}
fn inverse(a: &[Vec<f64>]) -> Option<Vec<Vec<f64>>> {
    let n = a.len();
    let mut cols = vec![];
    for i in 0..n {
        let mut e = vec![0.0; n];
        e[i] = 1.0;
        cols.push(solve(a, &e)?);
    }
    Some((0..n).map(|r| (0..n).map(|c| cols[c][r]).collect()).collect())
}
fn quad(v: &[f64], m: &[Vec<f64>]) -> f64 {
    let mut s = 0.0;
    for i in 0..v.len() {
        for j in 0..v.len() {
            s += v[i] * m[i][j] * v[j];
        }
    }
    s
}

/// Weighted block-jackknife covariance (Busing et al. 1999) of a vector statistic.
fn jack_cov(full: &[f64], loo: &[Vec<f64>], sizes: &[f64]) -> Vec<Vec<f64>> {
    let d = full.len();
    let total: f64 = sizes.iter().sum();
    let g = loo.len() as f64;
    let mut mean = vec![0.0; d];
    for (l, &m) in loo.iter().zip(sizes) {
        for k in 0..d {
            mean[k] += (1.0 - m / total) * l[k];
        }
    }
    let est: Vec<f64> = (0..d).map(|k| g * full[k] - mean[k]).collect();
    let mut cov = vec![vec![0.0; d]; d];
    for (l, &m) in loo.iter().zip(sizes) {
        if m <= 0.0 {
            continue;
        }
        let h = total / m;
        let ps: Vec<f64> = (0..d).map(|k| h * full[k] - (h - 1.0) * l[k] - est[k]).collect();
        for a in 0..d {
            for b in 0..d {
                cov[a][b] += ps[a] * ps[b] / (h - 1.0) / g;
            }
        }
    }
    cov
}

// ------------------------------------------------------------------ chi-square p-value
fn ln_gamma(x: f64) -> f64 {
    let c = [76.18009172947146, -86.50532032941677, 24.01409824083091, -1.231739572450155, 0.1208650973866179e-2, -0.5395239384953e-5];
    let mut y = x;
    let tmp = x + 5.5 - (x + 0.5) * (x + 5.5).ln();
    let mut ser = 1.000000000190015;
    for v in c {
        y += 1.0;
        ser += v / y;
    }
    -tmp + (2.5066282746310005 * ser / x).ln()
}
/// P(chisq >= x) with `dof` degrees of freedom
pub fn chisq_p(x: f64, dof: f64) -> f64 {
    if !(x > 0.0) {
        return 1.0;
    }
    let (a, x) = (dof / 2.0, x / 2.0);
    if x < a + 1.0 {
        // series for the lower incomplete gamma
        let (mut sum, mut del, mut ap) = (1.0 / a, 1.0 / a, a);
        for _ in 0..500 {
            ap += 1.0;
            del *= x / ap;
            sum += del;
            if del.abs() < sum.abs() * 1e-15 {
                break;
            }
        }
        (1.0 - sum * (-x + a * x.ln() - ln_gamma(a)).exp()).clamp(0.0, 1.0)
    } else {
        // continued fraction for the upper incomplete gamma
        let tiny = 1e-300;
        let mut b = x + 1.0 - a;
        let mut c = 1.0 / tiny;
        let mut d = 1.0 / b;
        let mut h = d;
        for i in 1..500 {
            let an = -(i as f64) * (i as f64 - a);
            b += 2.0;
            d = an * d + b;
            if d.abs() < tiny { d = tiny; }
            c = b + an / c;
            if c.abs() < tiny { c = tiny; }
            d = 1.0 / d;
            let del = d * c;
            h *= del;
            if (del - 1.0).abs() < 1e-15 {
                break;
            }
        }
        ((-x + a * x.ln() - ln_gamma(a)).exp() * h).clamp(0.0, 1.0)
    }
}

// ------------------------------------------------------------------ qpAdm
#[derive(Clone, Debug)]
pub struct ModelResult {
    pub sources: Vec<String>,
    pub weights: Vec<f64>,
    pub se: Vec<f64>,
    pub chisq: f64,
    pub dof: usize,
    pub p: f64,
    /// every weight between 0 and 1 (no slack for the standard errors)
    pub feasible: bool,
    pub n_snps: usize,
    pub n_blocks: usize,
}
impl ModelResult {
    pub fn passes(&self, st: &Settings) -> bool {
        self.p >= st.p_threshold && self.feasible
    }
}

/// Fit one qpAdm model on precomputed f2 blocks.
pub fn fit(f2: &F2, target: &str, sources: &[String], right: &[String], st: &Settings) -> Result<ModelResult, String> {
    let k = sources.len();
    let m = right.len();
    if k == 0 {
        return Err("add at least one source".into());
    }
    if m < k + 1 {
        return Err(format!("{k} source(s) need at least {} Right populations", k + 1));
    }
    let t = f2.idx(target);
    let s: Vec<usize> = sources.iter().map(|x| f2.idx(x)).collect();
    let r: Vec<usize> = right.iter().map(|x| f2.idx(x)).collect();
    let all: Vec<usize> = std::iter::once(t).chain(s.iter().copied()).chain(r.iter().copied()).collect();
    let sizes = f2.block_sizes(&all);
    let xmat = |skip: Option<usize>| -> Vec<Vec<f64>> {
        s.iter().map(|&si| (1..m).map(|j| f2.f4(t, si, r[0], r[j], skip)).collect()).collect()
    };
    let x = xmat(None);
    if x.iter().flatten().any(|v| !v.is_finite()) {
        return Err("some populations share no SNPs; try the allsnps option or other populations".into());
    }
    let loo: Vec<Vec<Vec<f64>>> = (0..f2.n_blocks).map(|b| xmat(Some(b))).collect();
    let mm = m - 1;
    // r(w) = X'w and its jackknife covariance
    let resid = |w: &[f64], x: &[Vec<f64>]| -> Vec<f64> { (0..mm).map(|j| (0..k).map(|i| w[i] * x[i][j]).sum()).collect() };
    let omega = |w: &[f64]| -> Vec<Vec<f64>> {
        let full = resid(w, &x);
        let l: Vec<Vec<f64>> = loo.iter().map(|xb| resid(w, xb)).collect();
        let mut c = jack_cov(&full, &l, &sizes);
        let tr = (0..mm).map(|i| c[i][i]).sum::<f64>() / mm as f64;
        for i in 0..mm {
            c[i][i] += st.fudge * tr.max(1e-12);
        }
        c
    };
    // w = M^-1 1 / (1' M^-1 1), M = X Q^-1 X'
    let weights_for = |oi: &[Vec<f64>], x: &[Vec<f64>]| -> Option<Vec<f64>> {
        let mut mmat = vec![vec![0.0; k]; k];
        for a in 0..k {
            for b in 0..k {
                let mut v = 0.0;
                for i in 0..mm {
                    for j in 0..mm {
                        v += x[a][i] * oi[i][j] * x[b][j];
                    }
                }
                mmat[a][b] = v;
            }
        }
        let tr = (0..k).map(|i| mmat[i][i]).sum::<f64>() / k as f64;
        for i in 0..k {
            mmat[i][i] += 1e-10 * tr.abs().max(1e-30);
        }
        let z = solve(&mmat, &vec![1.0; k])?;
        let sz: f64 = z.iter().sum();
        if sz.abs() < 1e-300 {
            return None;
        }
        Some(z.iter().map(|v| v / sz).collect())
    };
    let mut w = vec![1.0 / k as f64; k];
    let ident: Vec<Vec<f64>> = (0..mm).map(|i| (0..mm).map(|j| if i == j { 1.0 } else { 0.0 }).collect()).collect();
    if k > 1 {
        w = weights_for(&ident, &x).ok_or("the sources are too similar to tell apart with these Right populations")?;
    }
    let mut oi = inverse(&omega(&w)).ok_or("covariance could not be inverted")?;
    for _ in 0..12 {
        if k == 1 {
            break;
        }
        let nw = weights_for(&oi, &x).ok_or("the sources are too similar to tell apart with these Right populations")?;
        let moved = nw.iter().zip(&w).map(|(a, b)| (a - b).abs()).fold(0.0, f64::max);
        w = nw;
        oi = inverse(&omega(&w)).ok_or("covariance could not be inverted")?;
        if moved < 1e-7 {
            break;
        }
    }
    let rv = resid(&w, &x);
    let chisq = quad(&rv, &oi);
    let dof = mm - (k - 1);
    let p = chisq_p(chisq, dof as f64);
    // standard errors of the weights: jackknife with Q held fixed
    let se = if k > 1 {
        let lw: Vec<Vec<f64>> = loo.iter().map(|xb| weights_for(&oi, xb).unwrap_or_else(|| w.clone())).collect();
        let cov = jack_cov(&w, &lw, &sizes);
        (0..k).map(|i| cov[i][i].max(0.0).sqrt()).collect()
    } else {
        vec![0.0]
    };
    let feasible = w.iter().all(|&v| (-1e-9..=1.0 + 1e-9).contains(&v));
    Ok(ModelResult {
        sources: sources.to_vec(),
        weights: w,
        se,
        chisq,
        dof,
        p,
        feasible,
        n_snps: f2.n_snps,
        n_blocks: f2.n_blocks,
    })
}

/// qpWave-style check: can the Right set tell sources a and b apart?
/// p-value of "f4(a, b; Right_0, Right_j) = 0 for all j" (high p = they look the same).
pub fn same_to_right(f2: &F2, a: &str, b: &str, right: &[String], st: &Settings) -> f64 {
    let (ia, ib) = (f2.idx(a), f2.idx(b));
    let r: Vec<usize> = right.iter().map(|x| f2.idx(x)).collect();
    if r.len() < 2 {
        return 1.0;
    }
    let all: Vec<usize> = [ia, ib].into_iter().chain(r.iter().copied()).collect();
    let sizes = f2.block_sizes(&all);
    let v = |skip: Option<usize>| -> Vec<f64> { (1..r.len()).map(|j| f2.f4(ia, ib, r[0], r[j], skip)).collect() };
    let full = v(None);
    let loo: Vec<Vec<f64>> = (0..f2.n_blocks).map(|b| v(Some(b))).collect();
    let mut c = jack_cov(&full, &loo, &sizes);
    let d = full.len();
    let tr = (0..d).map(|i| c[i][i]).sum::<f64>() / d as f64;
    for i in 0..d {
        c[i][i] += st.fudge * tr.max(1e-12);
    }
    match inverse(&c) {
        Some(ci) => chisq_p(quad(&full, &ci), d as f64),
        None => 1.0,
    }
}

// ------------------------------------------------------------------ verdicts, presets, helpers
/// One plain-language line for a model. A pass only means the data can't rule the model out.
pub fn verdict(r: &ModelResult, st: &Settings) -> String {
    let p = fmt_p(r.p);
    if r.p >= st.p_threshold && r.feasible {
        format!("Not rejected (p = {p}). These sources can explain your target with this Right set; that makes the model possible, not proven.")
    } else if r.p >= st.p_threshold {
        format!("Not rejected (p = {p}), but a weight is below 0% or above 100%, so the mix isn't possible as stated. Try other sources.")
    } else {
        format!("Rejected (p = {p}). The target has ancestry these sources don't capture, or a source is a poor proxy.")
    }
}

/// Cautions about how much a result can be trusted: few SNPs, wide standard errors.
/// (level, text) like the pre-flight notes: 0 = info, 1 = warning.
pub fn reliability_notes(r: &ModelResult) -> Vec<(u8, String)> {
    let mut v = vec![];
    if r.n_snps < 200_000 {
        v.push((0, format!(
            "Low power: {}k SNPs, against about 1.2 million in published qpAdm work. With this little data qpAdm often can't reject a wrong model, so a pass is weak evidence. Compare several Right sets before trusting one.",
            r.n_snps / 1000)));
    }
    let worst = r.se.iter().cloned().fold(0.0f64, f64::max);
    if worst > 0.08 {
        v.push((1, format!(
            "The weights are uncertain: one has a standard error of ±{:.0} points, so the true share could be roughly {:.0} points either way.",
            worst * 100.0, 2.0 * worst * 100.0)));
    }
    v
}

pub fn fmt_p(p: f64) -> String {
    if p < 1e-4 { format!("{p:.1e}") } else { format!("{p:.4}") }
}

/// Ready-made Right sets (only populations that are in the pack).
pub const RIGHT_PRESETS: [(&str, &str, &[&str]); 6] = [
    ("Europe & West Eurasia", "Bronze Age and later Europeans, Anatolia, Caucasus",
        &["Mbuti", "Russia_Kostenki14", "Serbia_IronGates_Mesolithic", "Latvia_HG", "Turkey_N", "Iran_GanjDareh_N", "Russia_Samara_EBA_Yamnaya", "Israel_C", "Han_Shandong", "Karitiana", "Papuan"]),
    ("Middle East & North Africa", "Levant, Arabia, Egypt, Maghreb, Iran",
        &["Mbuti", "Yoruba", "Russia_Kostenki14", "Serbia_IronGates_Mesolithic", "Turkey_N", "Iran_GanjDareh_N", "Israel_C", "Russia_Samara_EBA_Yamnaya", "Han_Shandong", "Papuan"]),
    ("Central & South Asia", "Steppe, Iran plateau, Indus, India",
        &["Mbuti", "Russia_Kostenki14", "Latvia_HG", "Turkey_N", "Iran_GanjDareh_N", "Russia_Samara_EBA_Yamnaya", "Onge", "Han_Shandong", "Papuan", "Karitiana"]),
    ("East & North Asia", "China, Mongolia, Siberia, Japan",
        &["Mbuti", "Papuan", "Onge", "Karitiana", "China_Tianyuan", "Russia_Kostenki14", "Japan_HG_Jomon", "Mongolia_North_N", "Dai", "Iran_GanjDareh_N"]),
    ("Deep / global", "first look at anyone, any region",
        &["Mbuti", "Yoruba", "Russia_Kostenki14", "Papuan", "Onge", "Han_Shandong", "Karitiana", "China_Tianyuan", "Iran_GanjDareh_N", "Turkey_N"]),
    ("Hunter-gatherer Europe", "Mesolithic / Neolithic Europe",
        &["Mbuti", "Russia_Kostenki14", "Han_Shandong", "Papuan", "Karitiana", "Onge", "Iran_GanjDareh_N", "Israel_C", "Russia_Karelia_HG", "Ukraine_Mesolithic"]),
];

/// An R script that runs the same model in ADMIXTOOLS 2 (for checking with the standard tool).
pub fn admixtools2_script(target: &str, sources: &[String], right: &[String], st: &Settings) -> String {
    let q = |v: &[String]| v.iter().map(|s| format!("\"{s}\"")).collect::<Vec<_>>().join(", ");
    let mut s = String::new();
    s.push_str("# qpAdm model exported from genesloppa, for ADMIXTOOLS 2 (R)\n");
    s.push_str("# install: remotes::install_github(\"uqrmaie1/admixtools\")\nlibrary(admixtools)\n\n");
    s.push_str("prefix <- \"path/to/v66.p1_HO.aadr.patch.PUB\"   # AADR genotype files (.geno/.snp/.ind)\n");
    if target == MY_DNA {
        s.push_str("# \"My DNA\" must first be merged into the AADR files under this name (e.g. with plink / convertf)\n");
    }
    s.push_str(&format!("target <- \"{target}\"\nleft   <- c({})\nright  <- c({})\n\n", q(sources), q(right)));
    s.push_str(&format!(
        "f2_blocks <- f2_from_geno(prefix, pops = c(target, left, right), maxmiss = {}, adjust_pseudohaploid = TRUE, blgsize = {})\n",
        if st.allsnps { 1 } else { 0 },
        st.block_mb / 100.0
    ));
    s.push_str(&format!("res <- qpadm(f2_blocks, left, right, target, fudge = {})\nres$weights\nres$popdrop\n", st.fudge));
    s
}
