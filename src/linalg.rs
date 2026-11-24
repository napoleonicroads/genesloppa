//! Packed genotype storage + the small amount of linear algebra the converter needs.

pub const G25_DIMS: usize = 25;
pub type Coords = [f64; G25_DIMS];

/// rows = individuals, cols = SNPs; 2-bit codes: 0/1/2 = copies of allele1, 3 = missing
pub struct Packed {
    pub rows: usize,
    pub cols: usize,
    pub stride: usize,
    pub data: Vec<u8>,
}

impl Packed {
    pub fn new(rows: usize, cols: usize) -> Self {
        let stride = cols.div_ceil(4);
        Packed { rows, cols, stride, data: vec![0xFF; rows * stride] }
    }
    #[inline]
    pub fn get(&self, i: usize, j: usize) -> u8 {
        (self.data[i * self.stride + (j >> 2)] >> ((j & 3) * 2)) & 3
    }
    #[inline]
    pub fn set(&mut self, i: usize, j: usize, v: u8) {
        let b = &mut self.data[i * self.stride + (j >> 2)];
        let s = (j & 3) * 2;
        *b = (*b & !(3 << s)) | ((v & 3) << s);
    }
    #[inline]
    pub fn row(&self, i: usize) -> &[u8] {
        &self.data[i * self.stride..(i + 1) * self.stride]
    }
}

pub fn threads() -> usize {
    std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4)
}

/// Per SNP: samples observed and sum of their codes. Per sample: SNPs observed.
/// Integer counts, so splitting the rows across threads gives exactly the same totals.
pub fn count_codes(g: &Packed) -> (Vec<usize>, Vec<usize>, Vec<usize>) {
    let (n, m) = (g.rows, g.cols);
    let mut row_obs = vec![0usize; n];
    if n == 0 {
        return (vec![0; m], vec![0; m], row_obs);
    }
    let per = n.div_ceil(threads()).max(1);
    let parts: Vec<(Vec<usize>, Vec<usize>)> = std::thread::scope(|s| {
        let hs: Vec<_> = row_obs
            .chunks_mut(per)
            .enumerate()
            .map(|(t, ro)| {
                s.spawn(move || {
                    let (mut obs, mut sum) = (vec![0usize; m], vec![0usize; m]);
                    for (k, r) in ro.iter_mut().enumerate() {
                        let row = g.row(t * per + k);
                        for j in 0..m {
                            let c = (row[j >> 2] >> ((j & 3) * 2)) & 3;
                            if c < 3 {
                                obs[j] += 1;
                                sum[j] += c as usize;
                                *r += 1;
                            }
                        }
                    }
                    (obs, sum)
                })
            })
            .collect();
        hs.into_iter().map(|h| h.join().unwrap()).collect()
    });
    let (mut obs, mut sum) = (vec![0usize; m], vec![0usize; m]);
    for (o, s) in parts {
        for j in 0..m {
            obs[j] += o[j];
            sum[j] += s[j];
        }
    }
    (obs, sum, row_obs)
}

/// xorshift64* with Box-Muller for normals
pub struct Rng(pub u64);
impl Rng {
    pub fn next_u64(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545F4914F6CDD1D)
    }
    pub fn uniform(&mut self) -> f64 {
        ((self.next_u64() >> 11) as f64 + 0.5) / (1u64 << 53) as f64
    }
    pub fn gauss(&mut self) -> f64 {
        let (u, v) = (self.uniform(), self.uniform());
        (-2.0 * u.ln()).sqrt() * (2.0 * std::f64::consts::PI * v).cos()
    }
}

#[inline]
fn code(row: &[u8], j: usize) -> usize {
    ((row[j >> 2] >> ((j & 3) * 2)) & 3) as usize
}

// Rows of Z done together in z_times, so each row of M is loaded once per block instead of
// once per sample (M is tens of MB).
const ROW_BLOCK: usize = 8;
// rows per job in z_times
const ROW_JOB: usize = 32;
// SNP columns per job in zt_times; the output block stays in L2
const COL_BLOCK: usize = 128;

// Runs f(job index, chunk) over out.chunks_mut(size) on every core. Threads pull the next
// chunk when they're done, so the slower efficiency cores don't hold up the rest.
fn par_chunks<F: Fn(usize, &mut [f64]) + Sync>(out: &mut [f64], size: usize, f: F) {
    if out.is_empty() {
        return;
    }
    let jobs = std::sync::Mutex::new(out.chunks_mut(size).enumerate());
    std::thread::scope(|s| {
        for _ in 0..threads() {
            s.spawn(|| loop {
                let next = jobs.lock().unwrap().next();
                let Some((i, chunk)) = next else { break };
                f(i, chunk);
            });
        }
    });
}

// o += v[0]*a + v[1]*b + v[2]*c + v[3]*d for four outputs at once, each summed left to right.
// A zero v (missing genotype) adds an exact zero, which never changes o (o starts at +0.0 and
// the other matrix is finite), so this gives the same bits as skipping the missing ones.
// Doing four outputs per pass loads a, b, c, d once instead of four times.
#[inline(always)]
fn axpy4x4(o: &mut [f64], l: usize, v: &[[f64; 4]; 4], a: &[f64], b: &[f64], c: &[f64], d: &[f64]) {
    let (o0, rest) = o.split_at_mut(l);
    let (o1, rest) = rest.split_at_mut(l);
    let (o2, o3) = rest.split_at_mut(l);
    let (o3, a, b, c, d) = (&mut o3[..l], &a[..l], &b[..l], &c[..l], &d[..l]);
    let [v0, v1, v2, v3] = v;
    for e in 0..l {
        let (a, b, c, d) = (a[e], b[e], c[e], d[e]);
        o0[e] = o0[e] + v0[0] * a + v0[1] * b + v0[2] * c + v0[3] * d;
        o1[e] = o1[e] + v1[0] * a + v1[1] * b + v1[2] * c + v1[3] * d;
        o2[e] = o2[e] + v2[0] * a + v2[1] * b + v2[2] * c + v2[3] * d;
        o3[e] = o3[e] + v3[0] * a + v3[1] * b + v3[2] * c + v3[3] * d;
    }
}
#[inline(always)]
fn axpy4(o: &mut [f64], v: [f64; 4], a: &[f64], b: &[f64], c: &[f64], d: &[f64]) {
    for ((((o, a), b), c), d) in o.iter_mut().zip(a).zip(b).zip(c).zip(d) {
        *o = *o + v[0] * a + v[1] * b + v[2] * c + v[3] * d;
    }
}

/// Z (n x m, standardized on the fly via `lut`) times M (m x l) -> n x l.
/// Every output still sums over j = 0..m in order, so it matches the plain triple loop bit for bit.
pub fn z_times(g: &Packed, lut: &[[f64; 4]], m: &[f64], l: usize) -> Vec<f64> {
    let n = g.rows;
    let mut out = vec![0.0f64; n * l];
    if l == 0 {
        return out;
    }
    par_chunks(&mut out, ROW_JOB * l, |job, oc| {
        let r0 = job * ROW_JOB;
        for (bi, ob) in oc.chunks_mut(ROW_BLOCK * l).enumerate() {
            let rb = r0 + bi * ROW_BLOCK;
            let nb = ob.len() / l;
            let rows: [&[u8]; ROW_BLOCK] = std::array::from_fn(|k| if k < nb { g.row(rb + k) } else { &[][..] });
            let mut j = 0;
            while j + 4 <= g.cols {
                let (m0, m1, m2, m3) = (&m[j * l..(j + 1) * l], &m[(j + 1) * l..(j + 2) * l], &m[(j + 2) * l..(j + 3) * l], &m[(j + 3) * l..(j + 4) * l]);
                // j is a multiple of 4, so a row's four codes sit in one byte
                let (l0, l1, l2, l3) = (&lut[j], &lut[j + 1], &lut[j + 2], &lut[j + 3]);
                let vals = |k: usize| {
                    let byte = rows[k][j >> 2];
                    [l0[(byte & 3) as usize], l1[(byte >> 2 & 3) as usize], l2[(byte >> 4 & 3) as usize], l3[(byte >> 6) as usize]]
                };
                let mut k = 0;
                for o4 in ob.chunks_exact_mut(4 * l) {
                    let v = [vals(k), vals(k + 1), vals(k + 2), vals(k + 3)];
                    axpy4x4(o4, l, &v, m0, m1, m2, m3);
                    k += 4;
                }
                for orow in ob[k * l..].chunks_mut(l) {
                    axpy4(orow, vals(k), m0, m1, m2, m3);
                    k += 1;
                }
                j += 4;
            }
            while j < g.cols {
                let mj = &m[j * l..(j + 1) * l];
                for (k, orow) in ob.chunks_mut(l).enumerate() {
                    let v = lut[j][code(rows[k], j)];
                    if v != 0.0 {
                        for (o, x) in orow.iter_mut().zip(mj) {
                            *o += v * x;
                        }
                    }
                }
                j += 1;
            }
        }
    });
    out
}

/// Z transposed (m x n) times Q (n x l) -> m x l. Each job owns a block of SNPs, so there are
/// no partial sums to combine; every output sums over i = 0..n in order (same bits as the plain loop).
pub fn zt_times(g: &Packed, lut: &[[f64; 4]], q: &[f64], l: usize) -> Vec<f64> {
    let (n, mcols) = (g.rows, g.cols);
    let mut out = vec![0.0f64; mcols * l];
    if l == 0 {
        return out;
    }
    par_chunks(&mut out, COL_BLOCK * l, |job, ocb| {
        let c0 = job * COL_BLOCK;
        let c1 = c0 + ocb.len() / l;
        // c0 is a multiple of 4; columns past the last full group of four go one at a time
        let c4 = c0 + (c1 - c0) / 4 * 4;
        let mut i = 0;
        while i + 4 <= n {
            let rs = [g.row(i), g.row(i + 1), g.row(i + 2), g.row(i + 3)];
            let (q0, q1, q2, q3) = (&q[i * l..(i + 1) * l], &q[(i + 1) * l..(i + 2) * l], &q[(i + 2) * l..(i + 3) * l], &q[(i + 3) * l..(i + 4) * l]);
            let mut j = c0;
            for acc4 in ocb[..(c4 - c0) * l].chunks_exact_mut(4 * l) {
                let bytes = [rs[0][j >> 2], rs[1][j >> 2], rs[2][j >> 2], rs[3][j >> 2]];
                let v: [[f64; 4]; 4] = std::array::from_fn(|t| {
                    let lj = &lut[j + t];
                    std::array::from_fn(|r| lj[(bytes[r] >> (2 * t) & 3) as usize])
                });
                axpy4x4(acc4, l, &v, q0, q1, q2, q3);
                j += 4;
            }
            for (j, acc) in (c4..c1).zip(ocb[(c4 - c0) * l..].chunks_mut(l)) {
                let lj = &lut[j];
                axpy4(acc, [lj[code(rs[0], j)], lj[code(rs[1], j)], lj[code(rs[2], j)], lj[code(rs[3], j)]], q0, q1, q2, q3);
            }
            i += 4;
        }
        while i < n {
            let row = g.row(i);
            let qr = &q[i * l..(i + 1) * l];
            for (j, acc) in (c0..c1).zip(ocb.chunks_mut(l)) {
                let v = lut[j][code(row, j)];
                if v != 0.0 {
                    for (a, x) in acc.iter_mut().zip(qr) {
                        *a += v * x;
                    }
                }
            }
            i += 1;
        }
    });
    out
}

/// orthonormalize the l columns of a rowsxl row-major matrix (modified Gram-Schmidt, twice)
///
/// Works on a column-major copy so each pass streams contiguous memory, and fuses each
/// subtraction with the next dot product. The arithmetic and its order are unchanged, so the
/// result is bit-identical to the textbook loop over the row-major matrix.
pub fn orth(a: &mut [f64], rows: usize, l: usize) {
    if rows == 0 || l == 0 {
        return;
    }
    let mut cm = vec![0.0f64; rows * l];
    for i in 0..rows {
        for k in 0..l {
            cm[k * rows + i] = a[i * l + k];
        }
    }
    // the starting value f64's `Sum` uses, so manual folds match `.sum()` exactly
    let zero: f64 = std::iter::empty::<f64>().sum();
    for _ in 0..2 {
        for k in 0..l {
            let (done, rest) = cm.split_at_mut(k * rows);
            let ck = &mut rest[..rows];
            // d for p = 0, or the squared norm when k = 0
            let first = if k > 0 { &done[..rows] } else { &*ck };
            let mut acc = zero;
            for (x, y) in ck.iter().zip(first) {
                acc = acc + if k > 0 { x * y } else { x.powi(2) };
            }
            for p in 0..k {
                let d = acc;
                let cp = &done[p * rows..(p + 1) * rows];
                acc = zero;
                if p + 1 < k {
                    let cn = &done[(p + 1) * rows..(p + 2) * rows];
                    for ((x, y), z) in ck.iter_mut().zip(cp).zip(cn) {
                        *x -= d * y;
                        acc = acc + *x * z;
                    }
                } else {
                    for (x, y) in ck.iter_mut().zip(cp) {
                        *x -= d * y;
                        acc = acc + x.powi(2);
                    }
                }
            }
            let nrm = acc.sqrt().max(1e-300);
            for x in ck.iter_mut() {
                *x /= nrm;
            }
        }
    }
    for i in 0..rows {
        for k in 0..l {
            a[i * l + k] = cm[k * rows + i];
        }
    }
}

/// cyclic Jacobi eigen-decomposition of a symmetric lxl matrix -> (values desc, vectors as columns)
pub fn jacobi(mut a: Vec<f64>, l: usize) -> (Vec<f64>, Vec<f64>) {
    let mut v = vec![0.0; l * l];
    for i in 0..l {
        v[i * l + i] = 1.0;
    }
    for _ in 0..100 {
        let mut off = 0.0;
        for i in 0..l {
            for j in 0..l {
                if i != j {
                    off += a[i * l + j] * a[i * l + j];
                }
            }
        }
        if off < 1e-22 {
            break;
        }
        for p in 0..l {
            for q in (p + 1)..l {
                let apq = a[p * l + q];
                if apq.abs() < 1e-300 {
                    continue;
                }
                let theta = (a[q * l + q] - a[p * l + p]) / (2.0 * apq);
                let t = if theta == 0.0 { 1.0 } else { theta.signum() / (theta.abs() + (theta * theta + 1.0).sqrt()) };
                let c = 1.0 / (t * t + 1.0).sqrt();
                let s = t * c;
                for k in 0..l {
                    let (x, y) = (a[k * l + p], a[k * l + q]);
                    a[k * l + p] = c * x - s * y;
                    a[k * l + q] = s * x + c * y;
                }
                for k in 0..l {
                    let (x, y) = (a[p * l + k], a[q * l + k]);
                    a[p * l + k] = c * x - s * y;
                    a[q * l + k] = s * x + c * y;
                }
                for k in 0..l {
                    let (x, y) = (v[k * l + p], v[k * l + q]);
                    v[k * l + p] = c * x - s * y;
                    v[k * l + q] = s * x + c * y;
                }
            }
        }
    }
    let mut idx: Vec<usize> = (0..l).collect();
    idx.sort_by(|&x, &y| a[y * l + y].partial_cmp(&a[x * l + x]).unwrap_or(std::cmp::Ordering::Equal));
    let vals = idx.iter().map(|&i| a[i * l + i]).collect();
    let mut vecs = vec![0.0; l * l];
    for (nc, &oc) in idx.iter().enumerate() {
        for r in 0..l {
            vecs[r * l + nc] = v[r * l + oc];
        }
    }
    (vals, vecs)
}

/// ridge regression (intercept = feature 0, unpenalized), solved by Cholesky
pub fn ridge(f: &[Vec<f64>], y: &[Coords], rows: &[usize], lambda: f64) -> Vec<Coords> {
    let p = f[0].len();
    let mut a = vec![0.0; p * p];
    let mut b = vec![[0.0; G25_DIMS]; p];
    for &i in rows {
        for r in 0..p {
            for c in 0..p {
                a[r * p + c] += f[i][r] * f[i][c];
            }
            for d in 0..G25_DIMS {
                b[r][d] += f[i][r] * y[i][d];
            }
        }
    }
    for r in 1..p {
        a[r * p + r] += lambda;
    }
    let mut lo = vec![0.0; p * p];
    for i in 0..p {
        for j in 0..=i {
            let s = a[i * p + j] - (0..j).map(|k| lo[i * p + k] * lo[j * p + k]).sum::<f64>();
            lo[i * p + j] = if i == j { s.max(1e-300).sqrt() } else { s / lo[j * p + j] };
        }
    }
    let mut beta = vec![[0.0; G25_DIMS]; p];
    for d in 0..G25_DIMS {
        let mut z = vec![0.0; p];
        for i in 0..p {
            z[i] = (b[i][d] - (0..i).map(|k| lo[i * p + k] * z[k]).sum::<f64>()) / lo[i * p + i];
        }
        for i in (0..p).rev() {
            beta[i][d] = (z[i] - ((i + 1)..p).map(|k| lo[k * p + i] * beta[k][d]).sum::<f64>()) / lo[i * p + i];
        }
    }
    beta
}

pub fn predict(x: &[f64], beta: &[Coords]) -> Coords {
    let mut o = [0.0; G25_DIMS];
    for (xi, br) in x.iter().zip(beta) {
        for d in 0..G25_DIMS {
            o[d] += xi * br[d];
        }
    }
    o
}

pub fn dist(a: &Coords, b: &Coords) -> f64 {
    a.iter().zip(b).map(|(x, y)| (x - y).powi(2)).sum::<f64>().sqrt()
}

pub fn percentile(v: &mut [f64], p: f64) -> f64 {
    if v.is_empty() {
        return f64::NAN;
    }
    v.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    v[((v.len() - 1) as f64 * p).round() as usize]
}
