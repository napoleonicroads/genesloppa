//! Self-tests for the data tooling (run with `cargo test`).
use crate::{data, geo, model};

fn tmp(name: &str) -> String {
    std::env::temp_dir().join(name).to_string_lossy().into_owned()
}

/// minimal one-record polygon shapefile
fn write_shp(path: &str, ring: &[(f64, f64)]) {
    let n = ring.len();
    let content_len = 44 + 4 + 16 * n; // header fields + one part index + points
    let mut b = vec![0u8; 100];
    b[0..4].copy_from_slice(&9994u32.to_be_bytes());
    b[24..28].copy_from_slice((((100 + 8 + content_len) / 2) as u32).to_be_bytes().as_ref());
    b[28..32].copy_from_slice(&1000i32.to_le_bytes());
    b[32..36].copy_from_slice(&5i32.to_le_bytes());
    b.extend_from_slice(&1u32.to_be_bytes());
    b.extend_from_slice(&((content_len / 2) as u32).to_be_bytes());
    b.extend_from_slice(&5i32.to_le_bytes());
    b.extend_from_slice(&[0u8; 32]);
    b.extend_from_slice(&1i32.to_le_bytes());
    b.extend_from_slice(&(n as i32).to_le_bytes());
    b.extend_from_slice(&0i32.to_le_bytes());
    for &(x, y) in ring {
        b.extend_from_slice(&x.to_le_bytes());
        b.extend_from_slice(&y.to_le_bytes());
    }
    std::fs::write(path, b).unwrap();
}

#[test]
fn shapefile_roundtrip() {
    let (shp, map) = (tmp("t.shp"), tmp("t.g25map"));
    write_shp(&shp, &[(40.0, 38.0), (50.0, 38.0), (50.0, 44.0), (40.0, 44.0), (40.0, 38.0)]);
    assert_eq!(geo::shp_to_map(&shp, &map).unwrap(), 5);
    let w = geo::load_map(&map).unwrap();
    assert_eq!(w.rings.len(), 1);
    assert!((w.rings[0][0].0 as f64 - geo::merc_x(40.0)).abs() < 1e-3);
    assert!((geo::inv_merc_y(geo::merc_y(44.0)) - 44.0).abs() < 1e-9);
}

#[test]
fn places_and_heat() {
    let (anno, ind, out) = (tmp("t.anno"), tmp("t_ind.csv"), tmp("t_places.tsv"));
    std::fs::write(&anno, "Genetic ID\tMaster ID\tGroup ID\tLat.\tLong.\nI1.AG\tI1\tA\t40.0\t50.0\nI2.HO\tI2\tA\t42.0\t48.0\nI3.HO\tI3\tB\t50.0\t10.0\n").unwrap();
    std::fs::write(&ind, "PopA:I1,0\nPopA:I2,0\nPopB:I3,0\n").unwrap();
    assert_eq!(geo::build_places(&anno, &[ind], None, None, &out).unwrap(), 2);
    let places = geo::load_places(&out).unwrap();
    assert_eq!(places["PopA"], (41.0, 49.0));
    let sheet = "PopA,0.1,0.1\nPopB,0.3,0.3\nNoPlace,0.0,0.0";
    let h = geo::compute(&[0.1, 0.1], sheet, &places);
    assert_eq!(h.points.len(), 2);
    assert_eq!(h.points[0].name, "PopA");
    assert_eq!(h.rgba.len(), h.w * h.h * 4);
    assert!(h.rgba.chunks(4).any(|p| p[3] > 0));
}

#[test]
fn builtin_presets() {
    for i in 0..data::BUILTIN.len() {
        let t = data::builtin(i);
        let n = data::count_lines(&t);
        assert!(n >= 5, "{} has only {n}", data::BUILTIN[i]);
        model::parse_sheet(&t).unwrap();
    }
    assert!(data::builtin(0).contains("Kennewick Man"));
    assert!(data::builtin(1).contains("Funnelbeaker Culture"));
    assert!(data::builtin(2).contains("Kura-Araxes Culture"));
    assert!(data::builtin(5).contains("Norse Viking"));
    // no farmers or foragers leaking into the later periods
    for i in 2..6 {
        assert!(!data::builtin(i).contains("Neolithic") && !data::builtin(i).contains("Hunter-Gatherer"), "{}", data::BUILTIN[i]);
    }
}

#[test]
fn avatar_svg_layers() {
    use crate::avatar::{to_svg, Spec};
    use crate::traits::Texture;
    for (tex, long) in [(Texture::Straight, false), (Texture::Wavy, true), (Texture::Curly, false)] {
        let svg = to_svg(&Spec { skin: [78, 49, 34], hair: [31, 26, 23], eyes: [79, 134, 198], texture: tex, long_hair: long, freckles: 2 });
        assert!(svg.contains("#4e3122"), "skin colour applied");
        assert!(svg.contains("#4f86c6"), "eye colour applied");
        assert!(svg.matches("<path").count() > 30);
    }
}

// the fast kernels have to match the plain loops bit for bit
mod reference {
    use crate::linalg::Packed;
    pub fn z_times(g: &Packed, lut: &[[f64; 4]], m: &[f64], l: usize) -> Vec<f64> {
        let mut out = vec![0.0; g.rows * l];
        for i in 0..g.rows {
            for j in 0..g.cols {
                let v = lut[j][g.get(i, j) as usize];
                if v != 0.0 {
                    for x in 0..l {
                        out[i * l + x] += v * m[j * l + x];
                    }
                }
            }
        }
        out
    }
    pub fn zt_times(g: &Packed, lut: &[[f64; 4]], q: &[f64], l: usize) -> Vec<f64> {
        let mut out = vec![0.0; g.cols * l];
        for i in 0..g.rows {
            for j in 0..g.cols {
                let v = lut[j][g.get(i, j) as usize];
                if v != 0.0 {
                    for x in 0..l {
                        out[j * l + x] += v * q[i * l + x];
                    }
                }
            }
        }
        out
    }
    pub fn orth(a: &mut [f64], rows: usize, l: usize) {
        for _ in 0..2 {
            for k in 0..l {
                for p in 0..k {
                    let d: f64 = (0..rows).map(|i| a[i * l + k] * a[i * l + p]).sum();
                    for i in 0..rows {
                        a[i * l + k] -= d * a[i * l + p];
                    }
                }
                let nrm = (0..rows).map(|i| a[i * l + k].powi(2)).sum::<f64>().sqrt().max(1e-300);
                for i in 0..rows {
                    a[i * l + k] /= nrm;
                }
            }
        }
    }
}

fn bits(v: &[f64]) -> Vec<u64> {
    v.iter().map(|x| x.to_bits()).collect()
}

fn random_problem(rows: usize, cols: usize, seed: u64) -> (crate::linalg::Packed, Vec<[f64; 4]>) {
    let mut rng = crate::linalg::Rng(seed);
    let mut g = crate::linalg::Packed::new(rows, cols);
    for i in 0..rows {
        for j in 0..cols {
            g.set(i, j, (rng.next_u64() % 4) as u8);
        }
    }
    let lut = (0..cols)
        .map(|j| {
            let p = 0.05 + 0.9 * rng.uniform();
            let sd = (2.0 * p * (1.0 - p)).sqrt();
            let z = |x: f64| ((x - 2.0 * p) / sd) as f32 as f64;
            // every 7th SNP gets an exact zero to exercise the skip path
            [z(0.0), if j % 7 == 0 { 0.0 } else { z(1.0) }, z(2.0), 0.0]
        })
        .collect();
    (g, lut)
}

#[test]
fn z_and_zt_times_bit_identical() {
    for &(rows, cols, l) in &[(37, 101, 13), (8, 4, 1), (1, 9, 40), (64, 600, 40), (3, 1, 2)] {
        let (g, lut) = random_problem(rows, cols, rows as u64 * 31 + cols as u64);
        let mut rng = crate::linalg::Rng(99);
        let m: Vec<f64> = (0..cols * l).map(|_| rng.gauss()).collect();
        let q: Vec<f64> = (0..rows * l).map(|_| rng.gauss()).collect();
        assert_eq!(bits(&crate::linalg::z_times(&g, &lut, &m, l)), bits(&reference::z_times(&g, &lut, &m, l)), "z_times {rows}x{cols}x{l}");
        assert_eq!(bits(&crate::linalg::zt_times(&g, &lut, &q, l)), bits(&reference::zt_times(&g, &lut, &q, l)), "zt_times {rows}x{cols}x{l}");
    }
}

#[test]
fn orth_bit_identical() {
    for &(rows, l) in &[(500, 40), (41, 40), (7, 3), (1000, 1), (3, 5)] {
        let mut rng = crate::linalg::Rng(rows as u64 + 7);
        let a: Vec<f64> = (0..rows * l).map(|_| rng.gauss()).collect();
        let (mut x, mut y) = (a.clone(), a);
        crate::linalg::orth(&mut x, rows, l);
        reference::orth(&mut y, rows, l);
        assert_eq!(bits(&x), bits(&y), "orth {rows}x{l}");
    }
}

#[test]
fn raw_ascii_fast_path_matches_str_path() {
    // same records, but each line of the second copy ends in an EM SPACE (unicode whitespace
    // that trim() removes). That makes the line non-ASCII, so it takes the slow str path.
    let mut plain = String::from("# header\r\nrsid\tchromosome\tposition\tgenotype\r\n\x0b \t\r\n");
    let mut uni = String::from("# header\r\nrsid\tchromosome\tposition\tgenotype\r\n\x0b \t\u{2003}\r\n");
    let cells = [" rs1 ", "\"rs2\"", "rs3\x0b", "rs+4", "rs5x", "i6", "rs7", "\"\"", " \"rs9\" "];
    let chroms = ["1", " X ", "23", "22\x0c", "Y", "0", "7", "\"2\""];
    let gts = ["AG", "A", "\"CT\"", "--", "a g", "T\x0b", "GGG", "", "C"];
    for i in 0..30_000 {
        let rs = cells[i % cells.len()].replace("rs", &format!("rs{i}_")).replace('_', "");
        let mut line = format!("{rs}\t{}\t{i}\t{}", chroms[(i / 9) % chroms.len()], gts[(i / 72) % gts.len()]);
        if i % 5 == 0 {
            line = line.replace('\t', ",");
        }
        if i % 11 == 0 {
            line.push_str("\tAG"); // 5 fields: AncestryDNA layout
        }
        plain.push_str(&line);
        plain.push_str("\r\n");
        uni.push_str(&line);
        uni.push_str("\u{2003}\r\n");
    }
    let sorted = |m: crate::raw::RawGenotypes| {
        let mut v: Vec<_> = m.into_iter().collect();
        v.sort();
        v
    };
    let a = sorted(crate::raw::parse(plain.as_bytes()));
    let b = sorted(crate::raw::parse(uni.as_bytes()));
    assert!(a.len() > 1000, "test data should produce real records ({})", a.len());
    assert_eq!(a, b);
}

#[test]
fn builtin_map_and_places() {
    let w = crate::geo::load_map("/nonexistent/world.g25map").unwrap();
    assert!(w.rings.len() > 1000 && w.rings.iter().map(|r| r.len()).sum::<usize>() > 50_000);
    let p = crate::geo::load_places("/nonexistent/places.tsv").unwrap();
    assert!(p.len() > 2000);
    let (la, lo) = p["Lezgin"];
    assert!((41.0..44.0).contains(&la) && (46.0..50.0).contains(&lo));
}

#[test]
fn projector_recovers_reference_coordinates() {
    // projecting a reference sample through the fitted projector must land near its official
    // coordinates (it was part of the fit, so this checks plumbing and coding, not generalization)
    let dir = std::env::temp_dir().join(format!("g25_proj_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let pack_path = dir.join("p.g25pack");
    let synth = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../synth");
    if !synth.join("ref.ind").exists() {
        return; // synthetic AADR files not present on this machine
    }
    let aadr = synth.join("ref").to_string_lossy().into_owned();
    crate::pack::build(&aadr, &synth.join("g25.csv").to_string_lossy(), &pack_path.to_string_lossy(), 5, &[], &|_| {}).unwrap();
    let pack = crate::pack::Pack::open(&pack_path.to_string_lossy()).unwrap();
    let proj = crate::engine::Projector::fit(&pack, &|_| {}).unwrap();
    let cols: Vec<usize> = (0..pack.snps.len()).collect();
    let g = pack.genotypes(&cols).unwrap();
    let mut errs = vec![];
    for i in 0..pack.refs.len().min(40) {
        let codes: Vec<u8> = (0..cols.len()).map(|j| g.get(i, j)).collect();
        let x = proj.project(&codes, 1).unwrap();
        errs.push(crate::linalg::dist(&x, &pack.refs[i].coords));
    }
    errs.sort_by(|a, b| a.total_cmp(b));
    assert!(errs[errs.len() / 2] < 0.03, "median error {}", errs[errs.len() / 2]);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn heatmap_hides_diaspora_and_outliers_only() {
    use crate::geo::off_heatmap;
    for n in ["Ashkenazi_Poland", "Iraqi_Jew", "Mountain_Jew_Dagestan", "Roma_Madrid", "Parsi_India", "Afrikaner", "Kalmyk",
        "Karaite_Egypt", "Assyrian_o", "Tharu_o2", "Italy_Imperial_o3.SG", "Jew_Iraqi (est.)"] {
        assert!(off_heatmap(n), "{n} should be hidden");
    }
    for n in ["Romanian", "Russian_Kostroma", "Druze", "Samaritan", "Tatar_Kazan", "Lezgin", "Azerbaijani_Republic_Gabala",
        "Oroqen", "Mongolia_Xiongnu.SG", "Mayan (est.)"] {
        assert!(!off_heatmap(n), "{n} should stay");
    }
}

#[test]
fn distance_ranking() {
    let sh = crate::model::parse_sheet("Far,3,4\nNear,0.3,0.4\nSame,0,0\n").unwrap();
    let d = crate::model::distances(&[0.0, 0.0], &sh).unwrap();
    assert_eq!(d.iter().map(|r| r.0.as_str()).collect::<Vec<_>>(), ["Same", "Near", "Far"]);
    assert!((d[1].1 - 0.5).abs() < 1e-12 && (d[2].1 - 5.0).abs() < 1e-12);
    assert!(crate::model::distances(&[0.0], &sh).is_err());
    let txt = crate::model::render_distances("me", &d);
    assert!(txt.starts_with("Distance to:\tme\n0.00000000\tSame\n0.50000000\tNear"));
    // green at the top, red at the bottom, dark same-hue text
    let (g, gt) = crate::model::distance_colors(0.0);
    let (r, _) = crate::model::distance_colors(1.0);
    assert!(g[1] > g[0] && g[1] > g[2] && r[0] > r[1] && r[0] > r[2]);
    assert!(gt.iter().sum::<f64>() < 0.6);
    // the combined built-in pool parses, and a population is its own closest match
    let all = crate::model::parse_sheet(&format!("{}\n{}", crate::data::MODERN, crate::data::ANCIENT)).unwrap();
    let i = all.labels.len() / 3;
    let d = crate::model::distances(&all.rows[i], &all).unwrap();
    assert_eq!(d[0].1, 0.0);
    assert_eq!(d.len(), all.labels.len());
}

#[test]
fn npop_range_keeps_few_close_sources() {
    let src = model::parse_sheet(&data::builtin(3)).unwrap();
    // a 50/30/20 mix of three Iron Age populations
    let (a, b, c) = (3, 20, 35);
    let t: Vec<f64> = (0..src.rows[0].len()).map(|k| 0.5 * src.rows[a][k] + 0.3 * src.rows[b][k] + 0.2 * src.rows[c][k]).collect();
    let o = model::Options { npop: 3, npop_max: 5, ..Default::default() };
    let f = model::fit(&t, &src, &o).unwrap();
    assert!((3..=5).contains(&f.npop) && f.rows.len() <= 5, "{} rows, R{}P", f.rows.len(), f.npop);
    assert!(f.distance < 0.02, "distance {}", f.distance);
    let names: Vec<&str> = f.rows.iter().map(|r| r.0.as_str()).collect();
    assert!(names.contains(&src.labels[a].as_str()), "{names:?}");
}

#[test]
fn gradient_steepness() {
    use crate::model::{gradient_span, span_for_steepness, steepness_for_span, SPAN_GENTLE, SPAN_STEEP};
    assert!((span_for_steepness(0.0) - SPAN_GENTLE).abs() < 1e-12);
    assert!((span_for_steepness(1.0) - SPAN_STEEP).abs() < 1e-12);
    for k in 0..=10 {
        let s = k as f64 / 10.0;
        assert!((steepness_for_span(span_for_steepness(s)) - s).abs() < 1e-9);
    }
    // steeper slider = smaller gap to full red
    assert!(span_for_steepness(0.8) < span_for_steepness(0.2));
    let rows = vec![("a".to_string(), 0.02), ("b".to_string(), 0.05), ("c".to_string(), 0.09)];
    assert_eq!(gradient_span(&rows, None), (0.02, 0.09 - 0.02));
    assert_eq!(gradient_span(&rows[..2], None).1, 0.05 - 0.02); // fewer rows -> steeper
    assert_eq!(gradient_span(&rows, Some(0.03)), (0.02, 0.03));
}

#[test]
fn chisq_p_values() {
    use crate::qpadm::chisq_p;
    // known values: P(chisq > 3.841 | 1 dof) = 0.05, P(> 11.07 | 5) = 0.05, P(> 2.558 | 10) = 0.99
    assert!((chisq_p(3.841, 1.0) - 0.05).abs() < 1e-3);
    assert!((chisq_p(11.07, 5.0) - 0.05).abs() < 1e-3);
    assert!((chisq_p(2.558, 10.0) - 0.99).abs() < 1e-3);
    assert!(chisq_p(0.0, 3.0) == 1.0);
}

#[test]
fn qpadm_recovers_a_known_mixture() {
    use crate::qpadm::{self, Counts};
    use std::sync::Arc;
    let path = "dist/reference.g25pack";
    if !std::path::Path::new(path).exists() {
        return;
    }
    let pack = crate::pack::Pack::open(path).unwrap();
    let st = qpadm::Settings::default();
    let a = "Russia_Samara_EBA_Yamnaya".to_string();
    let b = "Turkey_N".to_string();
    let right: Vec<String> = ["Mbuti", "Russia_Kostenki14", "Serbia_IronGates_Mesolithic", "Iran_GanjDareh_N", "Han_Shandong", "Karitiana", "Papuan", "Israel_C", "Latvia_HG"]
        .iter().map(|s| s.to_string()).collect();
    let names: Vec<String> = [a.clone(), b.clone()].into_iter().chain(right.iter().cloned()).collect();
    let c = qpadm::counts(&pack, &names, None).unwrap();
    // synthetic target: 30% A + 70% B allele frequencies, 40 alleles per SNP where both are seen
    let ns = pack.snps.len();
    let mut t = Counts { name: "Synthetic".into(), samples: 20, alt: vec![0; ns], tot: vec![0; ns] };
    for j in 0..ns {
        if c[0].tot[j] > 0 && c[1].tot[j] > 0 {
            let p = 0.3 * c[0].alt[j] as f64 / c[0].tot[j] as f64 + 0.7 * c[1].alt[j] as f64 / c[1].tot[j] as f64;
            t.alt[j] = (p * 40.0).round() as u16;
            t.tot[j] = 40;
        }
    }
    let pops: Vec<Arc<Counts>> = std::iter::once(Arc::new(t)).chain(c.into_iter()).collect();
    let f2 = qpadm::f2_blocks(&pack, &pops, &st);
    let r = qpadm::fit(&f2, "Synthetic", &[a, b], &right, &st).unwrap();
    assert!((r.weights[0] - 0.3).abs() < 0.03, "weights {:?}", r.weights);
    assert!(r.p > 0.05, "p {}", r.p);
    // and a wrong single-source model is rejected
    let r1 = qpadm::fit(&f2, "Synthetic", &["Turkey_N".to_string()], &right, &st).unwrap();
    assert!(r1.p < 0.01, "p {}", r1.p);
}

#[test]
fn qpadm_your_own_populations() {
    use crate::qpadm;
    let path = "dist/reference.g25pack";
    if !std::path::Path::new(path).exists() {
        return;
    }
    let pack = crate::pack::Pack::open(path).unwrap();
    // names: reference populations and My DNA are taken
    assert!(qpadm::check_name(&pack, "Turkey_N").is_err());
    assert!(qpadm::check_name(&pack, qpadm::MY_DNA).is_err());
    assert_eq!(qpadm::check_name(&pack, "  Dad, Mum ").unwrap(), "Dad  Mum");

    // two raw files of a made-up person, heterozygous everywhere
    let mut g = crate::raw::RawGenotypes::default();
    for s in &pack.snps {
        g.insert(s.rsid, [s.a1, s.a2]);
    }
    let c = qpadm::add_from_raw(&pack, "Test_Family", &[g.clone(), g], None).unwrap();
    assert_eq!(c.samples, 2);
    let tf = qpadm::counts(&pack, &["Test_Family".to_string()], None).unwrap();
    assert!(tf[0].tot.iter().all(|&t| t == 4) && tf[0].alt.iter().all(|&a| a == 2));

    // a merged group pools the samples of its members
    let a = "Russia_Samara_EBA_Yamnaya".to_string();
    let b = "Turkey_N".to_string();
    let parts = qpadm::counts(&pack, &[a.clone(), b.clone()], None).unwrap();
    let m = qpadm::add_merged(&pack, "Test_Mix", &[a.clone(), b.clone()], None, None).unwrap();
    assert_eq!(m.samples, parts[0].samples + parts[1].samples);
    let mc = qpadm::counts(&pack, &["Test_Mix".to_string()], None).unwrap();
    assert!((0..pack.snps.len()).all(|j| mc[0].tot[j] == parts[0].tot[j] + parts[1].tot[j]));
    // and qpAdm models it as a mix of the two
    let right: Vec<String> = ["Mbuti", "Russia_Kostenki14", "Serbia_IronGates_Mesolithic", "Iran_GanjDareh_N", "Han_Shandong", "Karitiana", "Papuan", "Israel_C", "Latvia_HG"]
        .iter().map(|s| s.to_string()).collect();
    let names: Vec<String> = ["Test_Mix".to_string(), a.clone(), b.clone()].into_iter().chain(right.iter().cloned()).collect();
    let pops = qpadm::counts(&pack, &names, None).unwrap();
    let st = qpadm::Settings::default();
    let f2 = qpadm::f2_blocks(&pack, &pops, &st);
    let r = qpadm::fit(&f2, "Test_Mix", &[a, b], &right, &st).unwrap();
    assert!(r.weights.iter().all(|w| *w > 0.05 && *w < 0.95), "weights {:?}", r.weights);

    assert_eq!(qpadm::customs().len(), 2);
    qpadm::remove_custom("Test_Family");
    assert!(qpadm::custom("Test_Family").is_none() && qpadm::custom("Test_Mix").is_some());
}

/// Location audit: lists populations whose closest genetic matches are all placed far away,
/// which is how mislabelled AADR locations show up (Kurd_WGA sat in Algeria). Prints
/// candidates to check by hand; run with `cargo test --release audit_locations -- --ignored --nocapture`.
#[test]
#[ignore]
fn audit_locations() {
    use crate::{data, geo};
    let extras = "dist/extras";
    let places = geo::load_places_with_extras("", &format!("{extras}/extra_places.tsv")).unwrap();
    let mut sheet = format!("{}\n{}", data::MODERN, data::ANCIENT);
    for f in ["extra_modern.txt", "extra_ancient.txt"] {
        if let Ok(t) = std::fs::read_to_string(format!("{extras}/{f}")) {
            sheet.push('\n');
            sheet.push_str(&t);
        }
    }
    let pops: Vec<(String, Vec<f64>, (f64, f64))> = sheet
        .lines()
        .filter_map(|l| {
            let mut it = l.split(',');
            let name = it.next()?.trim().to_string();
            let v: Vec<f64> = it.filter_map(|x| x.trim().parse().ok()).collect();
            let at = *places.get(&name)?;
            (v.len() == 25 && !geo::off_heatmap(&name)).then_some((name, v, at))
        })
        .collect();
    let km = |a: (f64, f64), b: (f64, f64)| {
        let (la1, lo1, la2, lo2) = (a.0.to_radians(), a.1.to_radians(), b.0.to_radians(), b.1.to_radians());
        let h = ((la2 - la1) / 2.0).sin().powi(2) + la1.cos() * la2.cos() * ((lo2 - lo1) / 2.0).sin().powi(2);
        2.0 * 6371.0 * h.sqrt().asin()
    };
    let mut flagged = vec![];
    for (i, (name, v, at)) in pops.iter().enumerate() {
        let mut near: Vec<(f64, usize)> = pops
            .iter()
            .enumerate()
            .filter(|(j, _)| *j != i)
            .map(|(j, p)| (p.1.iter().zip(v).map(|(a, b)| (a - b).powi(2)).sum::<f64>().sqrt(), j))
            .collect();
        near.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap());
        let closest_km = near.iter().take(6).map(|&(_, j)| km(*at, pops[j].2)).fold(f64::MAX, f64::min);
        if closest_km > 2500.0 {
            flagged.push((closest_km, name.clone(), *at, pops[near[0].1].0.clone()));
        }
    }
    flagged.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap());
    println!("{} of {} placed populations have no close genetic match within 2500 km:", flagged.len(), pops.len());
    for (d, n, at, best) in &flagged {
        println!("{d:>6.0} km  {n} at ({:.2}, {:.2}); closest match {best}", at.0, at.1);
    }
}
