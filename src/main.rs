//! genesloppa — raw DNA → Global25 coordinates. Pure Rust, no dependencies.
//!
//!   g25app                                 open the native app window (macOS)
//!   g25app convert --raw FILE [--name N] [--pack reference.g25pack]
//!   g25app pack --aadr PREFIX --g25 SHEET.csv [--out reference.g25pack] [--per-pop 25]
//!   g25app health --raw FILE
//!   g25app traits --raw FILE
//!   g25app mapdata --shp ne_50m_admin_0_countries.shp [--out world.g25map]
//!   g25app places --anno X.anno --ind modern_ind.csv[,ancient_ind.csv] [--out places.tsv]
//!   g25app model --target TARGET.txt --sources SOURCES.txt [--npop N] [--cycles 1] [--adc 0.25 [--rc]] [--no-aggregate] [--zeroes]

mod avatar;
mod data;
mod extras;
mod engine;
mod geo;
mod health;
#[cfg(any(target_os = "macos", feature = "check-gui"))]
mod gui;
mod linalg;
mod logo;
mod model;
mod pack;
mod qpadm;
mod raw;
mod traits;
#[cfg(test)]
mod tests;

use std::collections::HashMap;
use std::process::exit;

fn flags(args: &[String]) -> HashMap<String, String> {
    let mut m = HashMap::new();
    let mut i = 0;
    while i + 1 < args.len() {
        m.insert(args[i].trim_start_matches("--").to_string(), args[i + 1].clone());
        i += 2;
    }
    m
}

fn expand(p: &str) -> String {
    match (p.strip_prefix("~/"), std::env::var("HOME")) {
        (Some(rest), Ok(h)) => format!("{h}/{rest}"),
        _ => p.to_string(),
    }
}

fn fail(e: String) -> ! {
    eprintln!("error: {e}");
    exit(1)
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    // Finder passes "-psn_..." to apps launched from older macOS; treat as no arguments
    let cmd = args.first().filter(|a| !a.starts_with("-psn")).map(String::as_str).unwrap_or("");
    let t0 = std::time::Instant::now();
    let log = |s: String| eprintln!("[{:>6.1}s] {s}", t0.elapsed().as_secs_f64());

    match cmd {
        "" => {
            #[cfg(target_os = "macos")]
            gui::run();
            #[cfg(not(target_os = "macos"))]
            fail("the window UI is macOS-only; use `g25app convert --raw FILE` here".into());
        }
        "pack" => {
            let f = flags(&args[1..]);
            let get = |k: &str| f.get(k).map(|s| expand(s)).unwrap_or_else(|| fail(format!("--{k} is required")));
            let per_pop = f.get("per-pop").and_then(|s| s.parse().ok()).unwrap_or(25);
            let out = f.get("out").cloned().unwrap_or_else(|| "reference.g25pack".into());
            let chips: Vec<String> = f.get("chip").map(|c| c.split(',').map(expand).collect()).unwrap_or_default();
            pack::build(&get("aadr"), &get("g25"), &out, per_pop, &chips, &log).unwrap_or_else(|e| fail(e));
        }
        "convert" => {
            let f = flags(&args[1..]);
            let raw_path = expand(f.get("raw").unwrap_or_else(|| fail("--raw is required".into())));
            let name = f.get("name").cloned().unwrap_or_else(|| "me".into());
            let pack_path = expand(f.get("pack").map(String::as_str).unwrap_or("reference.g25pack"));
            let settings = engine::Settings {
                pcs: f.get("pcs").and_then(|s| s.parse().ok()).unwrap_or(30),
                max_snps: f.get("max-snps").and_then(|s| s.parse().ok()).unwrap_or(60_000),
            };
            let user = raw::read_raw_file(&raw_path).unwrap_or_else(|e| fail(e));
            log(format!("{} usable SNPs in raw file", user.len()));
            let pack = pack::Pack::open(&pack_path).unwrap_or_else(|e| fail(e));
            log(format!("reference pack: {} samples, {} SNPs", pack.refs.len(), pack.snps.len()));
            let r = engine::convert(&pack, &user, &name, &settings, &|m| log(m.to_string())).unwrap_or_else(|e| fail(e));
            println!("{}\n\n{}", r.g25_line(), r.summary());
            eprintln!("(CV error median {:.4} / 90th pct {:.4}; allele check r = {:.3})", r.cv_median, r.cv_p90, r.allele_r);
        }
        "health" => {
            let f = flags(&args[1..]);
            let raw_path = expand(f.get("raw").unwrap_or_else(|| fail("--raw is required".into())));
            let g = raw::read_raw_file(&raw_path).unwrap_or_else(|e| fail(e));
            println!("{}", health::report(&g));
        }
        "avatar" => {
            // g25app avatar --raw FILE --out a.svg
            // g25app avatar --skin 0-7 --hair R,G,B --eyes R,G,B --texture straight|wavy|curly [--sex female] [--freckles 0-2] --out a.svg
            let f = flags(&args[1..]);
            let out = f.get("out").cloned().unwrap_or_else(|| "likeness.svg".into());
            let rgb = |k: &str, d: [u8; 3]| -> [u8; 3] {
                f.get(k).map(|v| {
                    let n: Vec<u8> = v.split(',').filter_map(|x| x.trim().parse().ok()).collect();
                    if n.len() == 3 { [n[0], n[1], n[2]] } else { fail(format!("--{k} wants R,G,B")) }
                }).unwrap_or(d)
            };
            let spec = if let Some(r) = f.get("raw") {
                let g = raw::read_raw_file(&expand(r)).unwrap_or_else(|e| fail(e));
                avatar::Spec::from_traits(&traits::predict(&g))
            } else {
                let skin_i: usize = f.get("skin").and_then(|v| v.parse().ok()).unwrap_or(4).min(7);
                avatar::Spec {
                    skin: traits::SKIN_TONES[skin_i].0,
                    hair: rgb("hair", [59, 42, 32]),
                    eyes: rgb("eyes", [94, 58, 32]),
                    texture: match f.get("texture").map(|s| s.as_str()) {
                        Some("wavy") => traits::Texture::Wavy,
                        Some("curly") => traits::Texture::Curly,
                        _ => traits::Texture::Straight,
                    },
                    long_hair: f.get("sex").map(|s| s == "female").unwrap_or(false),
                    freckles: f.get("freckles").and_then(|v| v.parse().ok()).unwrap_or(0),
                }
            };
            std::fs::write(&out, avatar::to_svg(&spec)).unwrap_or_else(|e| fail(e.to_string()));
            println!("wrote {out}");
        }
        "icon" => {
            // g25app icon --out genesloppa.icns   (or --out icon.svg for a vector copy)
            let f = flags(&args[1..]);
            let out = expand(f.get("out").map(String::as_str).unwrap_or("genesloppa.icns"));
            if out.ends_with(".svg") {
                std::fs::write(&out, logo::icon_svg()).unwrap_or_else(|e| fail(e.to_string()));
            } else {
                #[cfg(target_os = "macos")]
                gui::write_icns(&out).unwrap_or_else(|e| fail(e));
                #[cfg(not(target_os = "macos"))]
                fail("the .icns icon needs macOS; use --out icon.svg here".into());
            }
            println!("wrote {out}");
        }
        "locations" => {
            // g25app locations --anno a.janno,b.anno --out data/aadr_locations.tsv
            let f = flags(&args[1..]);
            let get = |k: &str| f.get(k).cloned().unwrap_or_else(|| fail(format!("--{k} is required")));
            let n = geo::build_locations(&get("anno"), &get("out")).unwrap_or_else(|e| fail(e));
            println!("wrote {} ({n} located samples)", get("out"));
        }
        "extras" => {
            // g25app extras --aadr PREFIX --pack reference.g25pack --locations data/aadr_locations.tsv --out DIR
            let f = flags(&args[1..]);
            let get = |k: &str| f.get(k).map(|v| expand(v)).unwrap_or_else(|| fail(format!("--{k} is required")));
            let t0 = std::time::Instant::now();
            let log = |m: String| eprintln!("[{:>7.1}s] {m}", t0.elapsed().as_secs_f64());
            let s = extras::build(&get("aadr"), &get("pack"), &get("locations"), &get("out"), &log).unwrap_or_else(|e| fail(e));
            println!(
                "estimated layer: {} modern + {} ancient populations added (of {} candidates); typical error {:.4} vs within-population spread {:.4}",
                s.modern, s.ancient, s.candidates, s.cv_median, s.within
            );
        }
        "traits" => {
            let f = flags(&args[1..]);
            let raw_path = expand(f.get("raw").unwrap_or_else(|| fail("--raw is required".into())));
            let g = raw::read_raw_file(&raw_path).unwrap_or_else(|e| fail(e));
            let t = traits::predict(&g);
            for x in &t.list {
                println!("{:<16} {:<45} [{}]\n{:16} {}", x.title, x.value, x.confidence.label(), "", x.detail);
            }
            println!("\nLikeness: {}", traits::summary(&t));
        }
        "mapdata" => {
            let f = flags(&args[1..]);
            let shp = expand(f.get("shp").unwrap_or_else(|| fail("--shp is required".into())));
            let out = f.get("out").cloned().unwrap_or_else(|| "world.g25map".into());
            let n = geo::shp_to_map(&shp, &out).unwrap_or_else(|e| fail(e));
            log(format!("wrote {out} ({n} points)"));
        }
        "qpadm" => {
            // g25app qpadm --pack reference.g25pack --target POP --left A,B --right R0,R1,... [--allsnps 1] [--raw FILE]
            let f = flags(&args[1..]);
            let pack = pack::Pack::open(&expand(f.get("pack").map(String::as_str).unwrap_or("reference.g25pack"))).unwrap_or_else(|e| fail(e));
            let list = |k: &str| -> Vec<String> { f.get(k).map(|v| v.split(',').map(|x| x.trim().to_string()).filter(|x| !x.is_empty()).collect()).unwrap_or_default() };
            let target = f.get("target").cloned().unwrap_or_else(|| fail("--target is required".into()));
            let (left, right) = (list("left"), list("right"));
            let st = qpadm::Settings { allsnps: f.get("allsnps").is_some(), ..Default::default() };
            let user = f.get("raw").map(|r| raw::read_raw_file(&expand(r)).unwrap_or_else(|e| fail(e)));
            let names: Vec<String> = std::iter::once(target.clone()).chain(left.iter().cloned()).chain(right.iter().cloned()).collect();
            let c = qpadm::counts(&pack, &names, user.as_ref()).unwrap_or_else(|e| fail(e));
            let f2 = qpadm::f2_blocks(&pack, &c, &st);
            log(format!("{} SNPs in {} blocks", f2.n_snps, f2.n_blocks));
            let r = qpadm::fit(&f2, &target, &left, &right, &st).unwrap_or_else(|e| fail(e));
            for (i, s) in r.sources.iter().enumerate() {
                println!("{:>7.1}% ± {:>4.1}  {s}", r.weights[i] * 100.0, r.se[i] * 100.0);
            }
            println!("chisq {:.2}  dof {}  p {}", r.chisq, r.dof, qpadm::fmt_p(r.p));
            println!("{}", qpadm::verdict(&r, &st));
        }
        "places" => {
            let f = flags(&args[1..]);
            let anno = expand(f.get("anno").unwrap_or_else(|| fail("--anno is required".into())));
            let sheets: Vec<String> = f.get("ind").unwrap_or_else(|| fail("--ind is required".into())).split(',').map(expand).collect();
            let out = f.get("out").cloned().unwrap_or_else(|| "places.tsv".into());
            let countries = f.get("countries").map(|c| expand(c));
            let provinces = f.get("provinces").map(|c| expand(c));
            let n = geo::build_places(&anno, &sheets, countries.as_deref(), provinces.as_deref(), &out).unwrap_or_else(|e| fail(e));
            log(format!("wrote {out} ({n} populations with coordinates)"));
        }
        "model" => {
            // boolean switches first, then key/value flags
            let rest: Vec<String> = args[1..].to_vec();
            let has = |f: &str| rest.iter().any(|a| a == f);
            let kv: Vec<String> = {
                let mut v = vec![];
                let mut i = 0;
                while i < rest.len() {
                    if ["--rc", "--no-aggregate", "--zeroes"].contains(&rest[i].as_str()) { i += 1; continue; }
                    v.push(rest[i].clone());
                    i += 1;
                }
                v
            };
            let f = flags(&kv);
            let read = |k: &str| {
                let p = expand(f.get(k).unwrap_or_else(|| fail(format!("--{k} is required"))));
                std::fs::read_to_string(&p).unwrap_or_else(|e| fail(format!("{p}: {e}")))
            };
            let sources = model::parse_sheet(&read("sources")).unwrap_or_else(|e| fail(format!("sources: {e}")));
            let targets = model::parse_sheet(&read("target")).unwrap_or_else(|e| fail(format!("target: {e}")));
            let o = model::Options {
                cycles: f.get("cycles").and_then(|s| s.parse().ok()).unwrap_or(1.0),
                adc: f.get("adc").and_then(|s| s.parse().ok()).unwrap_or(0.0),
                recalc: has("--rc"),
                npop: f.get("npop").and_then(|s| s.split('-').next()?.parse().ok()).unwrap_or(0),
                npop_max: f.get("npop").and_then(|s| s.split('-').nth(1)?.parse().ok()).unwrap_or(0),
                aggregate: !has("--no-aggregate"),
                zeroes: has("--zeroes"),
            };
            for (name, t) in targets.labels.iter().zip(&targets.rows) {
                let t0 = std::time::Instant::now();
                let r = model::fit(t, &sources, &o).unwrap_or_else(|e| fail(e));
                println!("{}", r.render(name, &o));
                eprintln!("(sources {}, populations {}, {} {} , {:.2}s)", r.sources, r.pops, if o.npop == 0 { "cycles" } else { "iterations" }, r.cycles_or_iters, t0.elapsed().as_secs_f64());
            }
        }
        "-h" | "--help" | "help" => println!("{}", include_str!("main.rs").lines().take(10).map(|l| l.trim_start_matches("//!")).collect::<Vec<_>>().join("\n")),
        other => fail(format!("unknown command `{other}` (try --help)")),
    }
}
