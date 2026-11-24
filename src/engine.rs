//! Raw genotypes + reference pack -> Global25-style coordinates.

use crate::linalg::*;
use crate::pack::Pack;
use crate::raw::RawGenotypes;
use std::collections::HashMap;

pub struct Settings {
    pub pcs: usize,
    pub max_snps: usize,
}
impl Default for Settings {
    fn default() -> Self {
        Settings { pcs: 30, max_snps: 60_000 }
    }
}

pub struct Report {
    pub name: String,
    pub coords: Coords,
    pub user_snps: usize,
    pub overlap: usize,
    pub used_snps: usize,
    pub refs: usize,
    pub pcs: usize,
    pub cv_median: f64,
    pub cv_p90: f64,
    pub within: f64,
    pub allele_r: f64,
    pub nearest: Vec<(String, f64)>,
}

impl Report {
    pub fn g25_line(&self) -> String {
        let nums: Vec<String> = self.coords.iter().map(|c| format!("{c:.6}")).collect();
        format!("{},{}", self.name, nums.join(","))
    }
    pub fn quality(&self) -> (&'static str, f64) {
        let r = self.cv_median / self.within;
        (if r < 0.5 { "Good" } else if r < 1.0 { "OK" } else { "Rough" }, r)
    }
    pub fn summary(&self) -> String {
        let (q, r) = self.quality();
        let mut s = format!(
            "Quality: {q} (error {r:.2}× normal within-population spread)\n\
             SNPs: {} in your file, {} shared with the reference, {} used\n\
             References: {} samples, {} principal components\n\nClosest populations:\n",
            self.user_snps, self.overlap, self.used_snps, self.refs, self.pcs
        );
        for (p, d) in &self.nearest {
            s.push_str(&format!("  {d:.4}   {p}\n"));
        }
        s
    }
}

fn complement(b: u8) -> u8 {
    match b {
        b'A' => b'T',
        b'T' => b'A',
        b'C' => b'G',
        b'G' => b'C',
        _ => 0,
    }
}

/// greedy spacing thinning: smallest gap that leaves <= max SNPs
fn thin(pos: &[(u8, u32)], max: usize) -> Vec<usize> {
    let pick = |gap: u32| -> Vec<usize> {
        let mut out = vec![];
        let (mut lc, mut lp) = (0u8, 0u32);
        for (i, &(c, p)) in pos.iter().enumerate() {
            if out.is_empty() || c != lc || p >= lp.saturating_add(gap) {
                out.push(i);
                lc = c;
                lp = p;
            }
        }
        out
    };
    if pos.len() <= max {
        return (0..pos.len()).collect();
    }
    let (mut lo, mut hi) = (0u32, 50_000_000u32);
    while lo < hi {
        let mid = lo + (hi - lo) / 2;
        if pick(mid).len() <= max { hi = mid } else { lo = mid + 1 }
    }
    pick(lo)
}

pub fn convert(pack: &Pack, user: &RawGenotypes, name: &str, s: &Settings, progress: &dyn Fn(&str)) -> Result<Report, String> {
    // 1. overlap + strand resolution; dosage = copies of the pack's allele1
    progress("Matching your SNPs to the reference…");
    let mut cols = vec![];
    let mut dose = vec![];
    for (j, snp) in pack.snps.iter().enumerate() {
        let Some(gt) = user.get(&snp.rsid) else { continue };
        let ok = |x: u8| x == snp.a1 || x == snp.a2;
        let g = if ok(gt[0]) && ok(gt[1]) {
            *gt
        } else {
            let c = [complement(gt[0]), complement(gt[1])];
            if ok(c[0]) && ok(c[1]) { c } else { continue }
        };
        cols.push(j);
        dose.push((g[0] == snp.a1) as u8 + (g[1] == snp.a1) as u8);
    }
    let overlap = cols.len();
    if overlap < 5_000 {
        return Err(format!("only {overlap} of your SNPs overlap the reference — this file can't be converted reliably"));
    }
    let pos: Vec<(u8, u32)> = cols.iter().map(|&j| (pack.snps[j].chrom, pack.snps[j].pos)).collect();
    let pick = thin(&pos, s.max_snps);
    let cols: Vec<usize> = pick.iter().map(|&k| cols[k]).collect();
    let mut user_codes: Vec<u8> = pick.iter().map(|&k| dose[k]).collect();
    let m = cols.len();

    progress("Loading reference genotypes…");
    let g = pack.genotypes(&cols)?;
    let n = g.rows;

    // 2. frequencies, allele-coding sanity check, standardization table
    let (col_obs, col_sum, row_obs) = count_codes(&g);
    let p: Vec<f64> = col_sum.iter().zip(&col_obs).map(|(&sum, &obs)| sum as f64 / (2.0 * obs.max(1) as f64)).collect();
    let allele_r = {
        let mx = user_codes.iter().map(|&c| c as f64).sum::<f64>() / m as f64;
        let my = p.iter().sum::<f64>() / m as f64;
        let (mut sxy, mut sxx, mut syy) = (0.0, 0.0, 0.0);
        for j in 0..m {
            let (dx, dy) = (user_codes[j] as f64 - mx, p[j] - my);
            sxy += dx * dy;
            sxx += dx * dx;
            syy += dy * dy;
        }
        sxy / (sxx * syy).sqrt()
    };
    if allele_r < 0.0 {
        user_codes.iter_mut().for_each(|c| *c = 2 - *c);
    }
    // z-scores are rounded to f32 but stored as f64, so the hot loops don't have to convert
    let lut: Vec<[f64; 4]> = p
        .iter()
        .map(|&pj| {
            let sd = (2.0 * pj * (1.0 - pj)).sqrt().max(1e-6);
            let z = |x: f64| ((x - 2.0 * pj) / sd) as f32 as f64;
            [z(0.0), z(1.0), z(2.0), 0.0]
        })
        .collect();

    // 3. randomized PCA on references (Halko et al.), 3 power iterations
    progress("Running PCA on the reference panel…");
    let k = s.pcs.min(n.saturating_sub(2)).max(2);
    let l = (k + 10).min(n);
    let mut rng = Rng(0x9E3779B97F4A7C15);
    let omega: Vec<f64> = (0..m * l).map(|_| rng.gauss()).collect();
    let mut q = z_times(&g, &lut, &omega, l);
    orth(&mut q, n, l);
    for _ in 0..3 {
        let mut w = zt_times(&g, &lut, &q, l);
        orth(&mut w, m, l);
        q = z_times(&g, &lut, &w, l);
        orth(&mut q, n, l);
    }
    let bt = zt_times(&g, &lut, &q, l);
    // B^T B is symmetric: sum the upper triangle and mirror it
    let mut c = vec![0.0; l * l];
    for j in 0..m {
        let r = &bt[j * l..(j + 1) * l];
        for x in 0..l {
            let rx = r[x];
            for y in x..l {
                c[x * l + y] += rx * r[y];
            }
        }
    }
    for x in 0..l {
        for y in 0..x {
            c[x * l + y] = c[y * l + x];
        }
    }
    let (vals, vecs) = jacobi(c, l);
    let sv: Vec<f64> = (0..k).map(|t| vals[t].max(1e-300).sqrt()).collect();
    let mut v = vec![0.0; m * k];
    for j in 0..m {
        let bj = &bt[j * l..(j + 1) * l];
        for t in 0..k {
            v[j * k + t] = (0..l).map(|x| bj[x] * vecs[x * l + t]).sum::<f64>() / sv[t];
        }
    }

    // 4. project references + user onto the fixed axes (rescaled for missing data)
    progress("Projecting you onto the reference axes…");
    let mut ref_scores = z_times(&g, &lut, &v, k);
    for i in 0..n {
        let obs = row_obs[i].max(1);
        let f = m as f64 / obs as f64;
        ref_scores[i * k..(i + 1) * k].iter_mut().for_each(|x| *x *= f);
    }
    let mut us = vec![0.0; k];
    for j in 0..m {
        let z = lut[j][user_codes[j] as usize];
        for t in 0..k {
            us[t] += z * v[j * k + t];
        }
    }

    // 5. map PCs -> official G25 coords (ridge), with 5-fold cross-validation
    progress("Calibrating to Global25…");
    let feats: Vec<Vec<f64>> = (0..n).map(|i| std::iter::once(1.0).chain(ref_scores[i * k..(i + 1) * k].iter().copied()).collect()).collect();
    let y: Vec<Coords> = pack.refs.iter().map(|r| r.coords).collect();
    let tr: f64 = feats.iter().map(|f| f[1..].iter().map(|x| x * x).sum::<f64>()).sum();
    let lambda = 1e-6 * tr / (n * k) as f64;

    let mut order: Vec<usize> = (0..n).collect();
    for i in (1..n).rev() {
        order.swap(i, (rng.next_u64() % (i as u64 + 1)) as usize);
    }
    let mut errs = vec![0.0; n];
    for fold in 0..5 {
        let train: Vec<usize> = order.iter().enumerate().filter(|(p, _)| p % 5 != fold).map(|(_, &i)| i).collect();
        let beta = ridge(&feats, &y, &train, lambda);
        for (_, &i) in order.iter().enumerate().filter(|(p, _)| p % 5 == fold) {
            errs[i] = dist(&predict(&feats[i], &beta), &y[i]);
        }
    }
    let mut groups: HashMap<u32, Vec<usize>> = HashMap::new();
    for (i, r) in pack.refs.iter().enumerate() {
        groups.entry(r.pop).or_default().push(i);
    }
    let mut within = vec![];
    for idx in groups.values().filter(|v| v.len() >= 3) {
        let mut mean = [0.0; G25_DIMS];
        for &i in idx {
            for d in 0..G25_DIMS {
                mean[d] += y[i][d] / idx.len() as f64;
            }
        }
        within.extend(idx.iter().map(|&i| dist(&y[i], &mean)));
    }

    let all: Vec<usize> = (0..n).collect();
    let beta = ridge(&feats, &y, &all, lambda);
    let ux: Vec<f64> = std::iter::once(1.0).chain(us).collect();
    let coords = predict(&ux, &beta);

    let mut nearest: Vec<(String, f64)> = pack.pops.iter().map(|p| (p.name.clone(), dist(&coords, &p.coords))).collect();
    nearest.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal));
    nearest.truncate(15);

    Ok(Report {
        name: name.to_string(),
        coords,
        user_snps: user.len(),
        overlap,
        used_snps: m,
        refs: n,
        pcs: k,
        cv_median: percentile(&mut errs.clone(), 0.5),
        cv_p90: percentile(&mut errs, 0.9),
        within: percentile(&mut within, 0.5),
        allele_r,
        nearest,
    })
}

// ---------------------------------------------------------------- batch projection
/// A converter fitted once on every SNP in the pack, for projecting many samples that come
/// straight from the AADR genotype file (same allele coding as the pack, so no strand step).
/// Unlike `convert`, the number of PCs and the ridge strength are chosen by cross-validation
/// against the official coordinates, to land as close to real G25 as the data allows.
pub struct Projector {
    lut: Vec<[f64; 4]>,
    v: Vec<f64>, // m x kmax
    k: usize,
    kmax: usize,
    beta: Vec<Coords>,
    pub cv_median: f64,
    pub within: f64,
    pub m: usize,
}

impl Projector {
    pub fn fit(pack: &Pack, log: &dyn Fn(String)) -> Result<Projector, String> {
        let m = pack.snps.len();
        let cols: Vec<usize> = (0..m).collect();
        let g = pack.genotypes(&cols)?;
        let n = g.rows;
        if n < 50 || m < 5_000 {
            return Err(format!("pack too small for projection ({n} samples, {m} SNPs)"));
        }
        let (col_obs, col_sum, row_obs) = count_codes(&g);
        let lut: Vec<[f64; 4]> = (0..m)
            .map(|j| {
                let pj = col_sum[j] as f64 / (2.0 * col_obs[j].max(1) as f64);
                let sd = (2.0 * pj * (1.0 - pj)).sqrt().max(1e-6);
                let z = |x: f64| ((x - 2.0 * pj) / sd) as f32 as f64;
                [z(0.0), z(1.0), z(2.0), 0.0]
            })
            .collect();
        // randomized PCA with enough components to try up to 50
        let kmax = 50.min(n.saturating_sub(2)).max(2);
        let l = (kmax + 10).min(n);
        log(format!("PCA on {n} reference samples × {m} SNPs…"));
        let mut rng = Rng(0x5EED_6250_0E57_1A7E);
        let omega: Vec<f64> = (0..m * l).map(|_| rng.gauss()).collect();
        let mut q = z_times(&g, &lut, &omega, l);
        orth(&mut q, n, l);
        for _ in 0..3 {
            let mut w = zt_times(&g, &lut, &q, l);
            orth(&mut w, m, l);
            q = z_times(&g, &lut, &w, l);
            orth(&mut q, n, l);
        }
        let bt = zt_times(&g, &lut, &q, l);
        let mut c = vec![0.0; l * l];
        for j in 0..m {
            let r = &bt[j * l..(j + 1) * l];
            for x in 0..l {
                for y in x..l {
                    c[x * l + y] += r[x] * r[y];
                }
            }
        }
        for x in 0..l {
            for y in 0..x {
                c[x * l + y] = c[y * l + x];
            }
        }
        let (vals, vecs) = jacobi(c, l);
        let mut v = vec![0.0; m * kmax];
        for j in 0..m {
            let bj = &bt[j * l..(j + 1) * l];
            for t in 0..kmax {
                v[j * kmax + t] = (0..l).map(|x| bj[x] * vecs[x * l + t]).sum::<f64>() / vals[t].max(1e-300).sqrt();
            }
        }
        let mut scores = z_times(&g, &lut, &v, kmax);
        for i in 0..n {
            let f = m as f64 / row_obs[i].max(1) as f64;
            scores[i * kmax..(i + 1) * kmax].iter_mut().for_each(|x| *x *= f);
        }
        let y: Vec<Coords> = pack.refs.iter().map(|r| r.coords).collect();
        let mut order: Vec<usize> = (0..n).collect();
        for i in (1..n).rev() {
            order.swap(i, (rng.next_u64() % (i as u64 + 1)) as usize);
        }
        let median = |mut e: Vec<f64>| {
            e.sort_by(|a, b| a.total_cmp(b));
            e[e.len() / 2]
        };
        // choose #PCs and ridge strength by 5-fold cross-validation
        let (mut best_k, mut best_lam, mut best_cv) = (kmax, 0.0, f64::MAX);
        for k in [20, 30, 40, 50].into_iter().filter(|&k| k <= kmax) {
            let feats: Vec<Vec<f64>> = (0..n).map(|i| std::iter::once(1.0).chain(scores[i * kmax..i * kmax + k].iter().copied()).collect()).collect();
            let tr: f64 = feats.iter().map(|f| f[1..].iter().map(|x| x * x).sum::<f64>()).sum();
            for scale in [1e-7, 1e-6, 1e-5, 1e-4, 1e-3] {
                let lambda = scale * tr / (n * k) as f64;
                let mut errs = vec![0.0; n];
                for fold in 0..5 {
                    let train: Vec<usize> = order.iter().enumerate().filter(|(p, _)| p % 5 != fold).map(|(_, &i)| i).collect();
                    let beta = ridge(&feats, &y, &train, lambda);
                    for (_, &i) in order.iter().enumerate().filter(|(p, _)| p % 5 == fold) {
                        errs[i] = dist(&predict(&feats[i], &beta), &y[i]);
                    }
                }
                let cv = median(errs);
                if cv < best_cv {
                    (best_k, best_lam, best_cv) = (k, lambda, cv);
                }
            }
        }
        let feats: Vec<Vec<f64>> = (0..n).map(|i| std::iter::once(1.0).chain(scores[i * kmax..i * kmax + best_k].iter().copied()).collect()).collect();
        let all: Vec<usize> = (0..n).collect();
        let beta = ridge(&feats, &y, &all, best_lam);
        // typical within-population spread, for scale
        let mut groups: HashMap<u32, Vec<usize>> = HashMap::new();
        for (i, r) in pack.refs.iter().enumerate() {
            groups.entry(r.pop).or_default().push(i);
        }
        let mut within = vec![];
        for idx in groups.values().filter(|v| v.len() >= 3) {
            let mut mean = [0.0; G25_DIMS];
            for &i in idx {
                for d in 0..G25_DIMS {
                    mean[d] += y[i][d] / idx.len() as f64;
                }
            }
            within.extend(idx.iter().map(|&i| dist(&y[i], &mean)));
        }
        let within = if within.is_empty() { f64::NAN } else { median(within) };
        log(format!("projector: {best_k} PCs, cross-validated median error {best_cv:.4} (typical within-population spread {within:.4})"));
        Ok(Projector { lut, v, k: best_k, kmax, beta, cv_median: best_cv, within, m })
    }

    /// Project one sample given its codes for every pack SNP (0..=2, 3 = missing).
    /// Returns None when fewer than `min_obs` SNPs are readable.
    pub fn project(&self, codes: &[u8], min_obs: usize) -> Option<Coords> {
        let mut us = vec![0.0; self.k];
        let mut obs = 0usize;
        for (j, &c) in codes.iter().enumerate().take(self.m) {
            if c < 3 {
                obs += 1;
            }
            let z = self.lut[j][c as usize];
            if z != 0.0 {
                let vj = &self.v[j * self.kmax..j * self.kmax + self.k];
                for (u, x) in us.iter_mut().zip(vj) {
                    *u += z * x;
                }
            }
        }
        if obs < min_obs.max(1) {
            return None;
        }
        let f = self.m as f64 / obs as f64;
        let x: Vec<f64> = std::iter::once(1.0).chain(us.iter().map(|u| u * f)).collect();
        Some(predict(&x, &self.beta))
    }
}
