//! The estimated heatmap layer: G25-style coordinates for AADR populations that have no
//! official Global25 coordinates, placed only where the official map has no nearby dot.
//!
//! Every AADR sample with a known location (from `data/aadr_locations.tsv`) that isn't one of
//! the pack's reference samples is projected with an `engine::Projector` (fitted once on all
//! pack SNPs, PCs and ridge strength chosen by cross-validation). Samples need at least half
//! of the pack SNPs readable; flagged outliers, contaminated samples, relatives and duplicates
//! are skipped; each population is the mean of its samples. Names carry " (est.)".

use crate::engine::Projector;
use crate::linalg::{Coords, G25_DIMS};
use crate::pack::{read_aadr_geno, Pack};
use std::collections::{HashMap, HashSet};
use std::io::{BufRead, BufReader, Write};

/// skip a populated place if an official dot of the same era is this close (km)
const NEAR_OFFICIAL_KM: f64 = 250.0;
/// keep estimated dots at least this far apart (km)
const MIN_SPACING_KM: f64 = 150.0;

fn km(a: (f64, f64), b: (f64, f64)) -> f64 {
    let (la1, lo1, la2, lo2) = (a.0.to_radians(), a.1.to_radians(), b.0.to_radians(), b.1.to_radians());
    let h = ((la2 - la1) / 2.0).sin().powi(2) + la1.cos() * la2.cos() * ((lo2 - lo1) / 2.0).sin().powi(2);
    2.0 * 6371.0 * h.sqrt().min(1.0).asin()
}

/// AADR group name without the data-type suffixes (.HO, .DG, .SG, ...)
fn clean_group(g: &str) -> String {
    let mut g = g.trim().to_string();
    while let Some(i) = g.rfind('.') {
        if matches!(g[i + 1..].to_lowercase().as_str(), "ho" | "dg" | "sg" | "ag" | "sdg" | "wgc" | "bgc") {
            g.truncate(i)
        } else {
            break;
        }
    }
    g
}

/// AADR flags problem samples in the group label: outliers ("_o", "_o2"), contamination,
/// low coverage, duplicates and relatives.
fn flagged(group: &str) -> bool {
    group.to_lowercase().split(['_', '.', '-']).any(|t| {
        (t.starts_with('o') && t[1..].chars().all(|c| c.is_ascii_digit()))
            || matches!(
                t,
                "outlier" | "ignore" | "contam" | "contaminated" | "lc" | "lowcov" | "dup" | "duplicate" | "rel" | "relative"
                    | "brother" | "sister" | "son" | "daughter" | "father" | "mother" | "sibling" | "possible" | "lowqual"
            )
    })
}

pub struct Summary {
    pub modern: usize,
    pub ancient: usize,
    pub candidates: usize,
    pub cv_median: f64,
    pub within: f64,
}

pub fn build(aadr: &str, pack_path: &str, locations: &str, out_dir: &str, log: &dyn Fn(String)) -> Result<Summary, String> {
    let pack = Pack::open(pack_path)?;
    let proj = Projector::fit(&pack, log)?;

    // pack SNP -> AADR SNP index
    let snp_file = std::fs::File::open(format!("{aadr}.snp")).map_err(|e| format!("{aadr}.snp: {e}"))?;
    let mut by_rs: HashMap<u32, usize> = HashMap::new();
    let mut nsnp = 0;
    for (i, l) in BufReader::new(snp_file).lines().map_while(Result::ok).enumerate() {
        nsnp = i + 1;
        if let Some(rs) = l.split_whitespace().next().and_then(|id| id.strip_prefix("rs")).and_then(|x| x.parse::<u32>().ok()) {
            by_rs.entry(rs).or_insert(i);
        }
    }
    let snp_idx: Vec<Option<usize>> = pack.snps.iter().map(|s| by_rs.get(&s.rsid).copied()).collect();
    let present: Vec<usize> = snp_idx.iter().flatten().copied().collect();
    if present.len() < pack.snps.len() / 2 {
        return Err("this AADR release doesn't match the reference pack's SNPs".into());
    }

    // locations, and the pack's own reference samples (already official)
    let mut loc: HashMap<String, (String, f64, f64, bool)> = HashMap::new();
    for l in std::fs::read_to_string(locations).map_err(|e| format!("{locations}: {e}"))?.lines() {
        let f: Vec<&str> = l.split('\t').collect();
        if f.len() < 5 {
            continue;
        }
        let (Ok(la), Ok(lo)) = (f[2].parse::<f64>(), f[3].parse::<f64>()) else { continue };
        let (la, lo) = crate::geo::AADR_GROUP_FIXES.iter().find(|g| g.0 == clean_group(f[1])).map_or((la, lo), |g| g.1);
        let v = (f[1].to_string(), la, lo, f[4] == "1");
        let id = f[0].to_lowercase();
        loc.entry(id.split('.').next().unwrap_or("").to_string()).or_insert(v.clone());
        loc.insert(id, v);
    }
    let official: HashSet<String> = pack.refs.iter().map(|r| r.label.rsplit_once(':').map_or(r.label.as_str(), |x| x.1).to_lowercase()).collect();

    let ind = std::fs::read_to_string(format!("{aadr}.ind")).map_err(|e| format!("{aadr}.ind: {e}"))?;
    let ids: Vec<String> = ind.lines().map(|l| l.split_whitespace().next().unwrap_or("").to_lowercase()).collect();
    let mut sel: Vec<usize> = vec![];
    let mut meta: Vec<(String, f64, f64, bool)> = vec![];
    for (i, id) in ids.iter().enumerate() {
        let base = id.split('.').next().unwrap_or("");
        if id.is_empty() || official.contains(id) || official.contains(base) {
            continue;
        }
        let Some(m) = loc.get(id).or_else(|| loc.get(base)) else { continue };
        if flagged(&m.0) {
            continue;
        }
        sel.push(i);
        meta.push(m.clone());
    }
    log(format!("{} located AADR samples without official G25 coordinates", sel.len()));

    // read their genotypes for the pack SNPs (columns missing from this release stay "missing")
    let geno = read_aadr_geno(&format!("{aadr}.geno"), &sel, &present, ids.len(), nsnp)?;
    let mut col_of = vec![usize::MAX; pack.snps.len()];
    let mut k = 0;
    for (j, s) in snp_idx.iter().enumerate() {
        if s.is_some() {
            col_of[j] = k;
            k += 1;
        }
    }

    // project each sample; average per population
    struct Acc {
        name: String,
        sum: Coords,
        n: usize,
        lat: Vec<f64>,
        lon: Vec<f64>,
        modern: usize,
    }
    let mut groups: HashMap<String, Acc> = HashMap::new();
    // project every sample, in parallel across cores
    let threads = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4);
    let chunk = meta.len().div_ceil(threads).max(1);
    let mut coords: Vec<Option<Coords>> = vec![None; meta.len()];
    std::thread::scope(|sc| {
        for (t, out) in coords.chunks_mut(chunk).enumerate() {
            let (geno, col_of, proj) = (&geno, &col_of, &proj);
            sc.spawn(move || {
                let mut codes = vec![3u8; col_of.len()];
                for (k0, o) in out.iter_mut().enumerate() {
                    let row = geno.row(t * chunk + k0);
                    for (j, c) in codes.iter_mut().enumerate() {
                        let k = col_of[j];
                        *c = if k == usize::MAX { 3 } else { (row[k >> 2] >> ((k & 3) * 2)) & 3 };
                    }
                    *o = proj.project(&codes, proj.m / 2);
                }
            });
        }
    });
    let mut projected = 0;
    for ((group, la, lo, modern), x) in meta.iter().zip(&coords) {
        let Some(x) = x else { continue };
        projected += 1;
        let name = clean_group(group);
        let a = groups.entry(name.to_lowercase()).or_insert(Acc { name, sum: [0.0; G25_DIMS], n: 0, lat: vec![], lon: vec![], modern: 0 });
        for d in 0..G25_DIMS {
            a.sum[d] += x[d];
        }
        a.n += 1;
        a.lat.push(*la);
        a.lon.push(*lo);
        a.modern += *modern as usize;
    }
    log(format!("{projected} samples had enough data; {} populations", groups.len()));

    // official dots per era (built-in sheets + built-in locations)
    let places = crate::geo::load_places("")?;
    let dots = |sheet: &str| -> Vec<(f64, f64)> {
        sheet.lines().filter_map(|l| places.get(l.split(',').next()?.trim()).copied()).collect()
    };
    let official_dots = [dots(crate::data::MODERN), dots(crate::data::ANCIENT)];
    let median = |mut v: Vec<f64>| {
        v.sort_by(|a, b| a.total_cmp(b));
        let n = v.len();
        if n % 2 == 1 { v[n / 2] } else { (v[n / 2 - 1] + v[n / 2]) / 2.0 }
    };
    let mut cands: Vec<(String, Coords, (f64, f64), usize, bool)> = groups
        .into_values()
        .map(|a| {
            let c: Coords = std::array::from_fn(|d| a.sum[d] / a.n as f64);
            let modern = a.modern * 2 > a.n;
            (a.name, c, (median(a.lat), median(a.lon)), a.n, modern)
        })
        .collect();
    let candidates = cands.len();
    // best-sampled populations claim their spot first
    cands.sort_by(|a, b| b.3.cmp(&a.3).then(a.0.cmp(&b.0)));
    let mut kept: [Vec<(f64, f64)>; 2] = [vec![], vec![]];
    let mut sheets = [String::new(), String::new()];
    let mut place_lines = String::new();
    for (name, c, at, n, modern) in cands {
        let era = if modern { 0 } else { 1 };
        if official_dots[era].iter().any(|&p| km(p, at) < NEAR_OFFICIAL_KM) || kept[era].iter().any(|&p| km(p, at) < MIN_SPACING_KM) {
            continue;
        }
        kept[era].push(at);
        let label = format!("{name} (est.)");
        let nums: Vec<String> = c.iter().map(|v| format!("{v:.6}")).collect();
        sheets[era].push_str(&format!("{label},{}\n", nums.join(",")));
        place_lines.push_str(&format!("{label}\t{:.4}\t{:.4}\t{n}\n", at.0, at.1));
    }
    std::fs::create_dir_all(out_dir).map_err(|e| e.to_string())?;
    for (file, body) in [("extra_modern.txt", &sheets[0]), ("extra_ancient.txt", &sheets[1]), ("extra_places.tsv", &place_lines)] {
        let p = format!("{out_dir}/{file}");
        std::fs::File::create(&p).and_then(|mut f| f.write_all(body.as_bytes())).map_err(|e| format!("{p}: {e}"))?;
    }
    Ok(Summary { modern: kept[0].len(), ancient: kept[1].len(), candidates, cv_median: proj.cv_median, within: proj.within })
}
