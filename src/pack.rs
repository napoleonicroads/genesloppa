//! The reference pack (`reference.g25pack`): everything the converter needs, built once from
//! the AADR and a Global25 sheet and shipped inside the app.
//!
//! Layout (little-endian):
//!   b"G25PACK1"
//!   u32 n_refs, u32 n_pops, u32 n_snps
//!   refs  x { str label, u32 pop_index, 25xf32 coords }
//!   pops  x { str name, u32 count, 25xf32 mean coords }   (all pops of the sheet, for "closest")
//!   snps  x { u32 rsid, u8 chrom, u32 pos, u8 allele1, u8 allele2 }
//!   genotypes: n_refs rows x ceil(n_snps/4) bytes (2-bit codes, see linalg::Packed)

use crate::linalg::{Coords, Packed, G25_DIMS};
use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::io::{BufRead, BufReader, BufWriter, Read, Seek, SeekFrom, Write};

const MAGIC: &[u8; 8] = b"G25PACK1";

pub struct RefSample {
    pub label: String,
    pub pop: u32,
    pub coords: Coords,
}
pub struct PopMean {
    pub name: String,
    pub count: u32,
    pub coords: Coords,
}
#[derive(Clone, Copy)]
pub struct Snp {
    pub rsid: u32,
    pub chrom: u8,
    pub pos: u32,
    pub a1: u8,
    pub a2: u8,
}

/// Metadata in memory; genotypes stay on disk and are read only for the SNPs a user overlaps.
pub struct Pack {
    pub path: String,
    pub refs: Vec<RefSample>,
    pub pops: Vec<PopMean>,
    pub snps: Vec<Snp>,
    geno_offset: u64,
    stride: usize,
}

// ------------------------------------------------------------ binary helpers
fn w_u32(w: &mut impl Write, v: u32) -> std::io::Result<()> {
    w.write_all(&v.to_le_bytes())
}
fn w_str(w: &mut impl Write, s: &str) -> std::io::Result<()> {
    let b = s.as_bytes();
    w.write_all(&(b.len().min(65535) as u16).to_le_bytes())?;
    w.write_all(&b[..b.len().min(65535)])
}
fn w_coords(w: &mut impl Write, c: &Coords) -> std::io::Result<()> {
    for x in c {
        w.write_all(&(*x as f32).to_le_bytes())?;
    }
    Ok(())
}
fn r_bytes<const N: usize>(r: &mut impl Read) -> std::io::Result<[u8; N]> {
    let mut b = [0u8; N];
    r.read_exact(&mut b)?;
    Ok(b)
}
fn r_u32(r: &mut impl Read) -> std::io::Result<u32> {
    Ok(u32::from_le_bytes(r_bytes::<4>(r)?))
}
fn r_str(r: &mut impl Read) -> std::io::Result<String> {
    let n = u16::from_le_bytes(r_bytes::<2>(r)?) as usize;
    let mut b = vec![0u8; n];
    r.read_exact(&mut b)?;
    Ok(String::from_utf8_lossy(&b).into_owned())
}
fn r_coords(r: &mut impl Read) -> std::io::Result<Coords> {
    let mut c = [0.0; G25_DIMS];
    for x in c.iter_mut() {
        *x = f32::from_le_bytes(r_bytes::<4>(r)?) as f64;
    }
    Ok(c)
}

impl Pack {
    pub fn open(path: &str) -> Result<Pack, String> {
        let f = File::open(path).map_err(|e| format!("reference pack {path}: {e}"))?;
        let mut r = BufReader::with_capacity(1 << 20, f);
        let io = |e: std::io::Error| format!("reference pack is damaged: {e}");
        if &r_bytes::<8>(&mut r).map_err(io)? != MAGIC {
            return Err("not a G25 reference pack".into());
        }
        let (nr, np, ns) = (r_u32(&mut r).map_err(io)?, r_u32(&mut r).map_err(io)?, r_u32(&mut r).map_err(io)?);
        let mut refs = Vec::with_capacity(nr as usize);
        for _ in 0..nr {
            refs.push(RefSample { label: r_str(&mut r).map_err(io)?, pop: r_u32(&mut r).map_err(io)?, coords: r_coords(&mut r).map_err(io)? });
        }
        let mut pops = Vec::with_capacity(np as usize);
        for _ in 0..np {
            pops.push(PopMean { name: r_str(&mut r).map_err(io)?, count: r_u32(&mut r).map_err(io)?, coords: r_coords(&mut r).map_err(io)? });
        }
        let mut snps = Vec::with_capacity(ns as usize);
        for _ in 0..ns {
            let b = r_bytes::<11>(&mut r).map_err(io)?;
            snps.push(Snp {
                rsid: u32::from_le_bytes([b[0], b[1], b[2], b[3]]),
                chrom: b[4],
                pos: u32::from_le_bytes([b[5], b[6], b[7], b[8]]),
                a1: b[9],
                a2: b[10],
            });
        }
        let geno_offset = r.stream_position().map_err(io)?;
        Ok(Pack { path: path.to_string(), refs, pops, snps, geno_offset, stride: (ns as usize).div_ceil(4) })
    }

    /// Raw packed genotype rows (all SNPs, 2-bit codes = copies of allele 1, 3 = missing) of
    /// the given reference samples (ascending indices).
    pub fn sample_rows(&self, idx: &[usize]) -> Result<Vec<Vec<u8>>, String> {
        let mut f = File::open(&self.path).map_err(|e| e.to_string())?;
        let mut out = Vec::with_capacity(idx.len());
        for &i in idx {
            f.seek(SeekFrom::Start(self.geno_offset + (i * self.stride) as u64)).map_err(|e| e.to_string())?;
            let mut row = vec![0u8; self.stride];
            f.read_exact(&mut row).map_err(|e| format!("reference pack truncated: {e}"))?;
            out.push(row);
        }
        Ok(out)
    }

    /// Genotypes of all reference samples at the given SNP indices (ascending).
    pub fn genotypes(&self, cols: &[usize]) -> Result<Packed, String> {
        let mut f = File::open(&self.path).map_err(|e| e.to_string())?;
        f.seek(SeekFrom::Start(self.geno_offset)).map_err(|e| e.to_string())?;
        let mut r = BufReader::with_capacity(1 << 22, f);
        let mut row = vec![0u8; self.stride];
        let mut m = Packed::new(self.refs.len(), cols.len());
        // a group of four columns that is exactly one byte of the pack can be copied whole
        let whole: Vec<bool> = cols.chunks(4).map(|q| q.len() == 4 && q[0] % 4 == 0 && q[1] == q[0] + 1 && q[2] == q[0] + 2 && q[3] == q[0] + 3).collect();
        for i in 0..self.refs.len() {
            r.read_exact(&mut row).map_err(|e| format!("reference pack truncated: {e}"))?;
            // build each output byte from four codes; unused trailing slots stay 3, like the
            // 0xFF fill Packed::new starts from
            let stride = m.stride;
            let out = &mut m.data[i * stride..(i + 1) * stride];
            for ((o, quad), &w) in out.iter_mut().zip(cols.chunks(4)).zip(&whole) {
                if w {
                    *o = row[quad[0] >> 2];
                    continue;
                }
                let mut b = 0xFFu8;
                for (s, &j) in quad.iter().enumerate() {
                    let c = (row[j >> 2] >> ((j & 3) * 2)) & 3;
                    b = (b & !(3 << (s * 2))) | (c << (s * 2));
                }
                *o = b;
            }
        }
        Ok(m)
    }
}

// ============================================================ building a pack from AADR

struct G25Sheet {
    label: Vec<String>,
    pop: Vec<String>,
    sid: Vec<String>,
    coords: Vec<Coords>,
}

fn read_g25(path: &str) -> Result<G25Sheet, String> {
    let f = File::open(path).map_err(|e| format!("{path}: {e}"))?;
    let mut g = G25Sheet { label: vec![], pop: vec![], sid: vec![], coords: vec![] };
    for line in BufReader::new(f).lines().map_while(Result::ok) {
        let mut it = line.trim().split(',');
        let Some(label) = it.next().map(str::trim).filter(|l| !l.is_empty()) else { continue };
        let nums: Vec<f64> = it.filter_map(|s| s.trim().parse().ok()).collect();
        if nums.len() < G25_DIMS {
            continue;
        }
        let mut c = [0.0; G25_DIMS];
        c.copy_from_slice(&nums[..G25_DIMS]);
        let (pop, sid) = label.rsplit_once(':').unwrap_or((label, label));
        g.label.push(label.to_string());
        g.pop.push(pop.to_string());
        g.sid.push(sid.to_string());
        g.coords.push(c);
    }
    if g.label.is_empty() {
        return Err(format!("{path}: no Global25 rows found"));
    }
    Ok(g)
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

/// Read AADR genotypes (packed GENO, transposed TGENO or text EIGENSTRAT) for refs x snps.
/// Output codes: copies of the .snp file's first allele, 3 = missing.
pub(crate) fn read_aadr_geno(path: &str, refs: &[usize], snps: &[usize], nind: usize, nsnp: usize) -> Result<Packed, String> {
    let mut f = File::open(path).map_err(|e| format!("{path}: {e}"))?;
    let mut head = [0u8; 48];
    f.read_exact(&mut head).map_err(|_| "geno file too short".to_string())?;
    f.seek(SeekFrom::Start(0)).map_err(|e| e.to_string())?;
    let mut m = Packed::new(refs.len(), snps.len());
    let bits = |buf: &[u8], k: usize| (buf[k >> 2] >> (6 - 2 * (k & 3))) & 3; // EIGENSOFT: high bits first

    let size = f.metadata().map_err(|e| e.to_string())?.len() as usize;
    // The header is either one full record (classic EIGENSOFT) or just 48 bytes (AADR v66
    // "patch" files) - tell them apart by the file size.
    let header_len = |rlen: usize, records: usize| -> Result<usize, String> {
        match size.checked_sub(records * rlen) {
            Some(h) if h == rlen || h == 48 => Ok(h),
            _ => Err(format!("geno file size doesn't match {records} records of {rlen} bytes — is the download complete?")),
        }
    };
    if head.starts_with(b"TGENO") {
        let rlen = nsnp.div_ceil(4).max(48);
        let hdr = header_len(rlen, nind)?;
        let mut buf = vec![0u8; rlen];
        for (r, &ind) in refs.iter().enumerate() {
            f.seek(SeekFrom::Start((hdr + ind * rlen) as u64)).map_err(|e| e.to_string())?;
            f.read_exact(&mut buf).map_err(|_| "truncated TGENO".to_string())?;
            for (k, &j) in snps.iter().enumerate() {
                m.set(r, k, bits(&buf, j));
            }
        }
    } else if head.starts_with(b"GENO") {
        let rlen = nind.div_ceil(4).max(48);
        let hdr = header_len(rlen, nsnp)?;
        let mut rd = BufReader::with_capacity(8 << 20, f);
        let mut buf = vec![0u8; rlen];
        rd.read_exact(&mut buf[..hdr]).map_err(|e| e.to_string())?;
        let mut next = 0;
        for j in 0..nsnp {
            if next == snps.len() {
                break;
            }
            rd.read_exact(&mut buf).map_err(|_| "truncated GENO".to_string())?;
            if snps[next] != j {
                continue;
            }
            for (r, &ind) in refs.iter().enumerate() {
                m.set(r, next, bits(&buf, ind));
            }
            next += 1;
        }
    } else {
        let mut next = 0;
        for (j, line) in BufReader::with_capacity(8 << 20, f).split(b'\n').enumerate() {
            if next == snps.len() {
                break;
            }
            if snps[next] != j {
                continue;
            }
            let line = line.map_err(|e| e.to_string())?;
            for (r, &ind) in refs.iter().enumerate() {
                m.set(r, next, match line.get(ind) { Some(b'0') => 0, Some(b'1') => 1, Some(b'2') => 2, _ => 3 });
            }
            next += 1;
        }
    }
    Ok(m)
}

/// Build `out` from AADR prefix (.../v62.0_HO_public, no extension) and a G25 individual sheet.
/// `chips`: raw DNA files whose SNP lists limit the pack to SNPs consumer chips actually test
/// (the Human Origins array shares only ~10% of its SNPs with them, so this shrinks the pack
/// ~10x without losing any SNP a user's file could match). Empty = keep every usable SNP.
pub fn build(aadr: &str, g25_path: &str, out: &str, per_pop: usize, chips: &[String], log: &dyn Fn(String)) -> Result<(), String> {
    let mut chip_snps: std::collections::HashSet<u32> = std::collections::HashSet::new();
    for c in chips {
        let g = crate::raw::read_raw_file(c)?;
        chip_snps.extend(g.keys().filter(|&&k| k & crate::raw::X_FLAG == 0));
    }
    if !chips.is_empty() {
        log(format!("{} SNPs on the given chip file(s)", chip_snps.len()));
    }
    let g = read_g25(g25_path)?;
    log(format!("G25 sheet: {} samples", g.label.len()));

    // match sheet samples -> AADR individuals
    let ind_file = File::open(format!("{aadr}.ind")).map_err(|e| format!("{aadr}.ind: {e}"))?;
    let (mut exact, mut base) = (HashMap::new(), HashMap::new());
    let mut nind = 0;
    for (i, line) in BufReader::new(ind_file).lines().map_while(Result::ok).enumerate() {
        nind = i + 1;
        let Some(id) = line.split_whitespace().next().map(str::to_lowercase) else { continue };
        base.entry(id.split('.').next().unwrap_or("").to_string()).or_insert(i);
        exact.insert(id, i);
    }
    let (mut used, mut per) = (HashSet::new(), HashMap::<&str, usize>::new());
    let mut matched: Vec<(usize, usize)> = vec![]; // (aadr index, sheet row)
    for r in 0..g.sid.len() {
        let k = g.sid[r].to_lowercase();
        if let Some(&i) = exact.get(&k).or_else(|| base.get(k.split('.').next().unwrap_or(""))) {
            let c = per.entry(g.pop[r].as_str()).or_insert(0);
            if *c < per_pop && used.insert(i) {
                *c += 1;
                matched.push((i, r));
            }
        }
    }
    matched.sort();
    log(format!("matched {} samples to AADR (max {per_pop} per population)", matched.len()));
    if matched.len() < 100 {
        return Err("too few matches — use the AADR HO dataset and a G25 *individual* sheet".into());
    }

    // candidate SNPs: autosomal rsIDs, single-base, not strand-ambiguous
    let snp_file = File::open(format!("{aadr}.snp")).map_err(|e| format!("{aadr}.snp: {e}"))?;
    let (mut cand_idx, mut cand) = (vec![], vec![]);
    let mut nsnp = 0;
    for (j, line) in BufReader::new(snp_file).lines().map_while(Result::ok).enumerate() {
        nsnp = j + 1;
        let f: Vec<&str> = line.split_whitespace().collect();
        if f.len() < 6 {
            continue;
        }
        let Some(rsid) = f[0].strip_prefix("rs").and_then(|x| x.parse::<u32>().ok()) else { continue };
        if !chip_snps.is_empty() && !chip_snps.contains(&rsid) {
            continue;
        }
        let Ok(chrom @ 1..=22) = f[1].parse::<u8>() else { continue };
        let (a1, a2) = (f[4].as_bytes(), f[5].as_bytes());
        if a1.len() != 1 || a2.len() != 1 || complement(a1[0]) == 0 || complement(a2[0]) == 0 || a1[0] == complement(a2[0]) {
            continue;
        }
        cand_idx.push(j);
        cand.push(Snp { rsid, chrom, pos: f[3].parse().unwrap_or(0), a1: a1[0], a2: a2[0] });
    }
    log(format!("{} usable SNPs of {nsnp}", cand.len()));

    let ref_ids: Vec<usize> = matched.iter().map(|m| m.0).collect();
    let geno = read_aadr_geno(&format!("{aadr}.geno"), &ref_ids, &cand_idx, nind, nsnp)?;
    log("genotypes read".into());

    // QC for a mix of modern and ancient DNA. Ancient samples are full of gaps, so the usual
    // "<5% missing" rule would throw out every SNP. Instead: drop samples with under half their
    // SNPs readable, then keep SNPs readable in at least half the rest, with MAF >= 1%.
    // Gaps left over are handled in the conversion (mean imputation, coverage rescaling).
    let (n0, m0) = (matched.len(), cand.len());
    let keep_ref: Vec<usize> = (0..n0)
        .filter(|&i| {
            let row = geno.row(i);
            (0..m0).filter(|&j| (row[j >> 2] >> ((j & 3) * 2)) & 3 != 3).count() * 2 >= m0
        })
        .collect();
    // one row-wise pass over the kept samples for per-SNP counts (cache-friendly)
    let (mut obs, mut sum) = (vec![0u32; m0], vec![0u32; m0]);
    for &i in &keep_ref {
        let row = geno.row(i);
        for j in 0..m0 {
            let c = (row[j >> 2] >> ((j & 3) * 2)) & 3;
            if c < 3 {
                obs[j] += 1;
                sum[j] += c as u32;
            }
        }
    }
    let keep_snp: Vec<usize> = (0..m0)
        .filter(|&j| {
            let p = sum[j] as f64 / (2.0 * obs[j].max(1) as f64);
            obs[j] as usize * 2 >= keep_ref.len() && p.min(1.0 - p) >= 0.01
        })
        .collect();
    if keep_snp.len() < 5_000 || keep_ref.len() < 100 {
        return Err(format!("only {} SNPs and {} samples passed quality control — check the AADR files", keep_snp.len(), keep_ref.len()));
    }
    log(format!("QC: {} SNPs, {} reference samples", keep_snp.len(), keep_ref.len()));

    // population means over the whole sheet (for "closest populations")
    let mut pop_index: HashMap<&str, usize> = HashMap::new();
    let mut pops: Vec<PopMean> = vec![];
    for (r, p) in g.pop.iter().enumerate() {
        let idx = *pop_index.entry(p).or_insert_with(|| {
            pops.push(PopMean { name: p.clone(), count: 0, coords: [0.0; G25_DIMS] });
            pops.len() - 1
        });
        pops[idx].count += 1;
        for d in 0..G25_DIMS {
            pops[idx].coords[d] += g.coords[r][d];
        }
    }
    for p in pops.iter_mut() {
        for d in 0..G25_DIMS {
            p.coords[d] /= p.count as f64;
        }
    }

    // write
    let io = |e: std::io::Error| format!("{out}: {e}");
    let mut w = BufWriter::with_capacity(1 << 22, File::create(out).map_err(io)?);
    w.write_all(MAGIC).map_err(io)?;
    w_u32(&mut w, keep_ref.len() as u32).map_err(io)?;
    w_u32(&mut w, pops.len() as u32).map_err(io)?;
    w_u32(&mut w, keep_snp.len() as u32).map_err(io)?;
    for &i in &keep_ref {
        let row = matched[i].1;
        w_str(&mut w, &g.label[row]).map_err(io)?;
        w_u32(&mut w, pop_index[g.pop[row].as_str()] as u32).map_err(io)?;
        w_coords(&mut w, &g.coords[row]).map_err(io)?;
    }
    for p in &pops {
        w_str(&mut w, &p.name).map_err(io)?;
        w_u32(&mut w, p.count).map_err(io)?;
        w_coords(&mut w, &p.coords).map_err(io)?;
    }
    for &j in &keep_snp {
        let s = cand[j];
        w.write_all(&s.rsid.to_le_bytes()).map_err(io)?;
        w.write_all(&[s.chrom]).map_err(io)?;
        w.write_all(&s.pos.to_le_bytes()).map_err(io)?;
        w.write_all(&[s.a1, s.a2]).map_err(io)?;
    }
    let mut row = Packed::new(1, keep_snp.len());
    for &i in &keep_ref {
        for (k, &j) in keep_snp.iter().enumerate() {
            row.set(0, k, geno.get(i, j));
        }
        w.write_all(row.row(0)).map_err(io)?;
    }
    w.flush().map_err(io)?;
    let size = std::fs::metadata(out).map(|m| m.len()).unwrap_or(0);
    log(format!("wrote {out} ({:.1} MB)", size as f64 / 1e6));
    Ok(())
}
