//! Heatmap data:
//! - `world.g25map`: country outlines converted from a Natural Earth shapefile (public domain)
//! - `places.tsv`: population -> latitude/longitude, from AADR/Poseidon annotations matched to
//!   Global25 individual sheets
//! - the heat raster: inverse-distance-weighted genetic distance, 12 steps from green to red
//!
//! Both files are compiled in from data/; a file with the same name next to the app wins.

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Write};

pub const MAP_W: f64 = 1000.0; // world width in map units (Mercator)
const LAT_TOP: f64 = 80.0;
const LAT_BOTTOM: f64 = -60.0;

pub fn merc_y(lat: f64) -> f64 {
    let l = lat.clamp(-85.0, 85.0).to_radians();
    MAP_W / (2.0 * std::f64::consts::PI) * (std::f64::consts::PI - (std::f64::consts::FRAC_PI_4 + l / 2.0).tan().ln())
}
pub fn inv_merc_y(y: f64) -> f64 {
    let t = std::f64::consts::PI - y * 2.0 * std::f64::consts::PI / MAP_W;
    (2.0 * t.exp().atan() - std::f64::consts::FRAC_PI_2).to_degrees()
}
pub fn merc_x(lon: f64) -> f64 {
    (lon + 180.0) / 360.0 * MAP_W
}

// ---------------------------------------------------------------- world.g25map
pub struct World {
    /// rings in map units (x, y), already projected
    pub rings: Vec<Vec<(f32, f32)>>,
}

/// Convert an ESRI .shp (polygon) file into the compact `G25MAP01` format.
pub fn shp_to_map(shp: &str, out: &str) -> Result<usize, String> {
    let mut b = vec![];
    std::fs::File::open(shp).and_then(|mut f| f.read_to_end(&mut b)).map_err(|e| format!("{shp}: {e}"))?;
    if b.len() < 100 || u32::from_be_bytes([b[0], b[1], b[2], b[3]]) != 9994 {
        return Err("not a shapefile".into());
    }
    let le_i32 = |o: usize| i32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]]);
    let le_f64 = |o: usize| f64::from_le_bytes(b[o..o + 8].try_into().unwrap());
    let mut rings: Vec<Vec<(f32, f32)>> = vec![];
    let mut o = 100;
    while o + 8 <= b.len() {
        let len = u32::from_be_bytes([b[o + 4], b[o + 5], b[o + 6], b[o + 7]]) as usize * 2;
        let c = o + 8;
        o = c + len;
        if c + 44 > b.len() || le_i32(c) == 0 {
            continue;
        }
        let st = le_i32(c);
        if st != 5 && st != 15 && st != 25 {
            continue; // polygons only
        }
        let (np, npts) = (le_i32(c + 36) as usize, le_i32(c + 40) as usize);
        let parts: Vec<usize> = (0..np).map(|i| le_i32(c + 44 + 4 * i) as usize).collect();
        let p0 = c + 44 + 4 * np;
        for (k, &s) in parts.iter().enumerate() {
            let e = if k + 1 < np { parts[k + 1] } else { npts };
            let ring: Vec<(f32, f32)> = (s..e)
                .map(|i| {
                    let (lon, lat) = (le_f64(p0 + 16 * i), le_f64(p0 + 16 * i + 8));
                    (merc_x(lon) as f32, merc_y(lat) as f32)
                })
                .collect();
            if ring.len() >= 3 {
                rings.push(ring);
            }
        }
    }
    let mut w = std::io::BufWriter::new(std::fs::File::create(out).map_err(|e| format!("{out}: {e}"))?);
    let io = |e: std::io::Error| e.to_string();
    w.write_all(b"G25MAP01").map_err(io)?;
    w.write_all(&(rings.len() as u32).to_le_bytes()).map_err(io)?;
    for r in &rings {
        w.write_all(&(r.len() as u32).to_le_bytes()).map_err(io)?;
        for &(x, y) in r {
            w.write_all(&x.to_le_bytes()).map_err(io)?;
            w.write_all(&y.to_le_bytes()).map_err(io)?;
        }
    }
    w.flush().map_err(io)?;
    Ok(rings.iter().map(|r| r.len()).sum())
}

/// Built in, so the heatmap works with no extra downloads: Natural Earth 50m country outlines
/// (public domain) and population locations from the AADR / Poseidon annotations.
static BUILTIN_MAP: &[u8] = include_bytes!("../data/world.g25map");
static BUILTIN_PLACES: &str = include_str!("../data/places.tsv");

/// The map at `path` if that file exists (an optional override), otherwise the built-in one.
pub fn load_map(path: &str) -> Result<World, String> {
    let mut b = vec![];
    match std::fs::File::open(path).and_then(|mut f| f.read_to_end(&mut b)) {
        Ok(_) => parse_map(&b).map_err(|e| format!("{path}: {e}")),
        Err(_) => parse_map(BUILTIN_MAP),
    }
}

fn parse_map(b: &[u8]) -> Result<World, String> {
    if b.len() < 12 || &b[..8] != b"G25MAP01" {
        return Err("not a G25 map file".into());
    }
    let u = |o: usize| u32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]]) as usize;
    let f = |o: usize| f32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]]);
    let n = u(8);
    let mut o = 12;
    let mut rings = Vec::with_capacity(n);
    for _ in 0..n {
        let m = u(o);
        o += 4;
        if o + 8 * m > b.len() {
            return Err("map file truncated".into());
        }
        rings.push((0..m).map(|i| (f(o + 8 * i), f(o + 8 * i + 4))).collect());
        o += 8 * m;
    }
    Ok(World { rings })
}

// ---------------------------------------------------------------- places.tsv
/// Lowercase, with common Latin diacritics folded ("Şanlıurfa" -> "sanliurfa").
fn fold(s: &str) -> String {
    s.chars()
        .map(|c| match c {
            'á' | 'à' | 'â' | 'ä' | 'ã' | 'å' | 'ā' | 'ą' | 'Á' | 'À' | 'Â' | 'Ä' | 'Å' => 'a',
            'ç' | 'č' | 'ć' | 'Ç' | 'Č' => 'c',
            'é' | 'è' | 'ê' | 'ë' | 'ē' | 'ę' | 'ě' | 'É' | 'È' => 'e',
            'í' | 'ì' | 'î' | 'ï' | 'ı' | 'ī' | 'İ' | 'Í' => 'i',
            'ñ' | 'ń' | 'ň' => 'n',
            'ó' | 'ò' | 'ô' | 'ö' | 'õ' | 'ø' | 'ō' | 'ő' | 'Ó' | 'Ö' => 'o',
            'ú' | 'ù' | 'û' | 'ü' | 'ū' | 'ű' | 'ů' | 'Ú' | 'Ü' => 'u',
            'ş' | 'š' | 'ś' | 'Ş' | 'Š' => 's',
            'ž' | 'ź' | 'ż' | 'Ž' => 'z',
            'ğ' | 'Ğ' => 'g',
            'ł' | 'Ł' => 'l',
            'ř' => 'r',
            'ý' => 'y',
            c => c,
        })
        .flat_map(char::to_lowercase)
        .collect()
}

/// Named places from a Natural Earth `.dbf` (countries or provinces): folded name -> every point
/// with that name. Countries use their label point, provinces their latitude/longitude.
pub fn named_points(dbf: &str) -> Result<HashMap<String, Vec<(f64, f64)>>, String> {
    let b = std::fs::read(dbf).map_err(|e| format!("{dbf}: {e}"))?;
    if b.len() < 32 {
        return Err(format!("{dbf}: not a dBASE file"));
    }
    let n = u32::from_le_bytes([b[4], b[5], b[6], b[7]]) as usize;
    let (hlen, rlen) = (u16::from_le_bytes([b[8], b[9]]) as usize, u16::from_le_bytes([b[10], b[11]]) as usize);
    let mut fields = vec![]; // (name, offset in record, length)
    let (mut o, mut off) = (32, 1);
    while o + 32 <= hlen && b[o] != 0x0D {
        let name = String::from_utf8_lossy(&b[o..o + 11]).trim_end_matches('\0').to_uppercase();
        let len = b[o + 16] as usize;
        fields.push((name, off, len));
        off += len;
        o += 32;
    }
    let col = |n: &str| fields.iter().find(|f| f.0 == n).map(|f| (f.1, f.2));
    let (lx, ly) = match (col("LABEL_X"), col("LABEL_Y"), col("LONGITUDE"), col("LATITUDE")) {
        (Some(x), Some(y), _, _) | (_, _, Some(x), Some(y)) => (x, y),
        _ => return Err(format!("{dbf}: no coordinates")),
    };
    let names: Vec<(usize, usize)> = ["NAME", "NAME_LONG", "ADMIN", "NAME_EN", "NAME_ALT", "GN_NAME", "WOE_NAME"].iter().filter_map(|n| col(n)).collect();
    // provinces carry their country in ADMIN; for them only the province's own names count
    let is_admin1 = col("LATITUDE").is_some() && col("LABEL_X").is_none();
    let admin = col("ADMIN");
    let mut out: HashMap<String, Vec<(f64, f64)>> = HashMap::new();
    for r in 0..n {
        let rec = hlen + r * rlen;
        if rec + rlen > b.len() || b[rec] == b'*' {
            continue;
        }
        // fields are padded with spaces or NUL bytes depending on the writer
        let text = |(o, l): (usize, usize)| String::from_utf8_lossy(&b[rec + o..rec + o + l]).trim_matches(|c: char| c == '\0' || c.is_whitespace()).to_string();
        let (Ok(x), Ok(y)) = (text(lx).parse::<f64>(), text(ly).parse::<f64>()) else { continue };
        let mut seen = std::collections::HashSet::new();
        for &c in &names {
            if is_admin1 && Some(c) == admin {
                continue;
            }
            for nm in text(c).split('|') {
                let nm = fold(nm.trim());
                if !nm.is_empty() && seen.insert(nm.clone()) {
                    out.entry(nm).or_default().push((y, x));
                }
            }
        }
    }
    Ok(out)
}

/// One point for a name, or None when it's ambiguous (places more than ~5 degrees apart).
fn resolve(points: Option<&Vec<(f64, f64)>>) -> Option<(f64, f64)> {
    let p = points?;
    let (la, lo) = p.iter().fold((0.0, 0.0), |a, q| (a.0 + q.0, a.1 + q.1));
    let c = (la / p.len() as f64, lo / p.len() as f64);
    p.iter().all(|q| (q.0 - c.0).abs() < 5.0 && (q.1 - c.1).abs() < 5.0).then_some(c)
}

/// Nationality words used in G25 population names -> Natural Earth country name.
const DEMONYMS: &[(&str, &str)] = &[
    ("afghan", "afghanistan"), ("afrikaner", "south africa"), ("albanian", "albania"), ("algerian", "algeria"),
    ("armenian", "armenia"), ("austrian", "austria"), ("azerbaijani", "azerbaijan"), ("bangladeshi", "bangladesh"),
    ("belarusian", "belarus"), ("belgian", "belgium"), ("bosnian", "bosnia and herzegovina"), ("bulgarian", "bulgaria"),
    ("croatian", "croatia"), ("cypriot", "cyprus"), ("czech", "czechia"), ("danish", "denmark"), ("dutch", "netherlands"),
    ("egyptian", "egypt"), ("emirati", "united arab emirates"), ("english", "united kingdom"), ("estonian", "estonia"),
    ("ethiopian", "ethiopia"), ("finnish", "finland"), ("french", "france"), ("georgian", "georgia"), ("german", "germany"),
    ("greek", "greece"), ("greenlander", "greenland"), ("hungarian", "hungary"), ("icelandic", "iceland"),
    ("indonesian", "indonesia"), ("iranian", "iran"), ("iraqi", "iraq"), ("irish", "ireland"), ("italian", "italy"),
    ("jordanian", "jordan"), ("kuwaiti", "kuwait"), ("kyrgyz", "kyrgyzstan"), ("lao", "laos"), ("latvian", "latvia"),
    ("lebanese", "lebanon"), ("libyan", "libya"), ("lithuanian", "lithuania"), ("macedonian", "north macedonia"),
    ("malayan", "malaysia"), ("maltese", "malta"), ("moldovan", "moldova"), ("mongolian", "mongolia"),
    ("montenegrin", "montenegro"), ("moroccan", "morocco"), ("nepali", "nepal"), ("norwegian", "norway"),
    ("omani", "oman"), ("pakistani", "pakistan"), ("palestinian", "palestine"), ("polish", "poland"),
    ("portuguese", "portugal"), ("qatari", "qatar"), ("romanian", "romania"), ("saudi", "saudi arabia"),
    ("scottish", "united kingdom"), ("serbian", "serbia"), ("sinhala", "sri lanka"), ("slovak", "slovakia"),
    ("slovakian", "slovakia"), ("slovenian", "slovenia"), ("spanish", "spain"), ("sudanese", "sudan"),
    ("swedish", "sweden"), ("swiss", "switzerland"), ("syrian", "syria"), ("tajik", "tajikistan"),
    ("tunisian", "tunisia"), ("turkish", "turkey"), ("turkmen", "turkmenistan"), ("ukrainian", "ukraine"),
    ("uzbek", "uzbekistan"), ("welsh", "united kingdom"), ("yemenite", "yemen"), ("yemeni", "yemen"),
];
/// Directions and other generic words: they name provinces in many countries, so they never
/// locate a population on their own.
const GENERIC: &[&str] = &[
    "north", "south", "east", "west", "northeast", "northwest", "southeast", "southwest", "northern",
    "southern", "eastern", "western", "central", "centre", "center", "upper", "lower", "high", "low",
    "highland", "highlands", "lowland", "mountain", "mountains", "coast", "coastal", "island", "islands",
    "republic", "old", "new", "valley", "urban", "rural", "jew", "jewish", "muslim", "christian", "sunni",
    "shia", "tribal", "rest", "mixed", "other", "lake", "river", "plain", "steppe", "desert", "border",
];

/// Hand-checked locations for well-documented groups whose labels name no place the automatic
/// steps can resolve (approximate homeland centres). Vague labels such as "Amerindian_North"
/// are deliberately absent.
const MANUAL: &[(&str, f64, f64)] = &[
    ("Saami_Kola", 68.0, 35.0),          // Lovozero, Kola Peninsula
    ("Vepsian", 61.0, 35.5),             // Vepsian uplands (Leningrad/Vologda/Karelia)
    ("Ingrian", 59.5, 29.5),             // Ingria, west of St Petersburg
    ("Shetlandic", 60.3, -1.3),          // Shetland
    ("Polish_Kashubian", 54.3, 18.0),    // Kashubia, Pomerania
    ("Russian_Pinega", 64.7, 43.4),      // Pinega, Arkhangelsk Oblast
    ("Russian_Voronez", 51.7, 39.2),     // Voronezh
    ("German_Erlangen", 49.6, 11.0),     // Erlangen, Bavaria
    ("Greenlander_East", 65.6, -37.6),   // Tasiilaq
    ("Greenlander_West", 64.2, -51.7),   // Nuuk
    ("Aymara", -16.5, -68.5),            // Lake Titicaca altiplano
    ("Huichol", 22.0, -104.0),           // Sierra Madre Occidental, Jalisco/Nayarit
    ("Wichi", -23.5, -62.5),             // Gran Chaco, Salta/Formosa
    ("Yukpa", 10.0, -72.8),              // Serranía del Perijá
    ("Cachi", -25.1, -66.2),             // Cachí, Salta
    ("Colla", -23.0, -65.8),             // Puna of Jujuy/Salta
];

/// Too big for one label point to mean anything: never used as a fallback location.
const TOO_BIG: &[&str] = &["russia", "china", "india", "united states of america", "canada", "brazil", "australia", "kazakhstan"];

/// Build population coordinates: G25 individual sheets ("Pop:SampleID,...") x sample lat/long from
/// one or more annotation tables (AADR `.anno`, or Poseidon `.janno`; comma-separated paths).
/// Earlier tables win when a sample appears in several.
pub fn build_places(annos: &str, sheets: &[String], countries: Option<&str>, provinces: Option<&str>, out: &str) -> Result<usize, String> {
    let countries = match countries {
        Some(d) => named_points(d)?,
        None => HashMap::new(),
    };
    let provinces = match provinces {
        Some(d) => named_points(d)?,
        None => HashMap::new(),
    };
    // sample ID -> every annotated (lat, lon, group) with that ID: IDs are not unique across
    // studies (modern chip IDs like "349_R02C02" recur), so the group decides which one is meant
    let mut coords: HashMap<String, Vec<(f64, f64, String)>> = HashMap::new();
    // group name (lowercase, without .HO/.DG/.SG/.AG suffixes) -> (sum lat, sum lon, n)
    let mut groups: HashMap<String, (f64, f64, usize)> = HashMap::new();
    let norm_group = |g: &str| -> String {
        let mut g = g.trim().to_lowercase();
        while let Some(i) = g.rfind('.') {
            if matches!(&g[i + 1..], "ho" | "dg" | "sg" | "ag" | "sdg" | "wgc" | "bgc") { g.truncate(i) } else { break }
        }
        g
    };
    for anno in annos.split(',').map(str::trim).filter(|a| !a.is_empty()) {
        let f = std::fs::File::open(anno).map_err(|e| format!("{anno}: {e}"))?;
        let mut lines = BufReader::new(f).lines().map_while(Result::ok);
        let Some(header) = lines.next() else { continue };
        let cols: Vec<String> = header.split('\t').map(|s| s.trim().to_lowercase()).collect();
        // exact names first (".anno": "Lat."/"Long."; ".janno": "Latitude"/"Longitude"), then any match
        let find = |exact: &[&str], key: &str| {
            cols.iter().position(|c| exact.contains(&c.as_str())).or_else(|| cols.iter().position(|c| c.contains(key)))
        };
        let (Some(lat_c), Some(lon_c)) = (find(&["lat.", "latitude"], "lat"), find(&["long.", "longitude"], "long")) else {
            continue; // a table without coordinates contributes nothing
        };
        let alt_c = cols.iter().position(|c| c == "alternative_ids");
        let grp_c = cols.iter().position(|c| c == "group_name" || c == "group id" || c == "group_id");
        for l in lines {
            let f: Vec<&str> = l.split('\t').collect();
            if f.len() <= lat_c.max(lon_c) {
                continue;
            }
            let (Ok(la), Ok(lo)) = (f[lat_c].trim().parse::<f64>(), f[lon_c].trim().parse::<f64>()) else { continue };
            let group = grp_c.and_then(|c| f.get(c)).map(|g| g.trim().to_string()).unwrap_or_default();
            if let Some(g) = grp_c.and_then(|c| f.get(c)) {
                for name in g.split(';') {
                    let e = groups.entry(norm_group(name)).or_insert((0.0, 0.0, 0));
                    e.0 += la;
                    e.1 += lo;
                    e.2 += 1;
                }
            }
            let mut ids = vec![f[0].trim().to_lowercase()];
            if let Some(a) = alt_c.and_then(|c| f.get(c)) {
                ids.extend(a.split(';').map(|x| x.trim().to_lowercase()).filter(|x| !x.is_empty() && x != "n/a"));
            }
            for id in ids {
                let base = id.split('.').next().unwrap_or("").to_string();
                if base != id {
                    coords.entry(base).or_default().push((la, lo, group.clone()));
                }
                coords.entry(id).or_default().push((la, lo, group.clone()));
            }
        }
    }
    // population -> (lat, lon, samples): the median of the matched samples, so a few rough
    // placeholder coordinates (AADR puts the 1000 Genomes Tuscans at 40N 9E) can't drag it off;
    // the fallbacks below add a single location with n = 1
    let mut samples: HashMap<String, Vec<(f64, f64)>> = HashMap::new();
    let mut acc: HashMap<String, (f64, f64, usize)> = HashMap::new();
    for s in sheets {
        let t = std::fs::read_to_string(s).map_err(|e| format!("{s}: {e}"))?;
        for line in t.lines() {
            let label = line.split(',').next().unwrap_or("");
            let Some((pop, sid)) = label.rsplit_once(':') else { continue };
            let k = sid.to_lowercase();
            let cands = coords.get(&k).or_else(|| coords.get(k.split('.').next().unwrap_or("")));
            // accept a sample only if its annotated group shares a word with the population name
            // ("Yamnaya_Samara" ~ "Russia_Samara_EBA_Yamnaya"), or when no group is recorded
            let words = |x: &str| -> Vec<String> {
                fold(x).split(|c: char| !c.is_alphanumeric()).filter(|w| w.len() >= 4).map(str::to_string).collect()
            };
            let pw = words(pop);
            let hit = cands.and_then(|v| {
                v.iter().find(|(_, _, g)| {
                    let gw = words(g);
                    gw.is_empty() || gw.iter().any(|w| pw.contains(w)) // nothing to compare -> trust the ID
                })
            });
            if let Some(&(la, lo, _)) = hit {
                samples.entry(pop.to_string()).or_default().push((la, lo));
            }
        }
    }
    let median = |mut v: Vec<f64>| -> f64 {
        v.sort_by(|a, b| a.total_cmp(b));
        let n = v.len();
        if n % 2 == 1 { v[n / 2] } else { (v[n / 2 - 1] + v[n / 2]) / 2.0 }
    };
    for (pop, v) in samples {
        let n = v.len();
        let la = median(v.iter().map(|p| p.0).collect());
        let lo = median(v.iter().map(|p| p.1).collect());
        acc.insert(pop, (la, lo, n));
    }
    // populations with no matched sample: fall back to the annotation group of exactly the same
    // name (not a shortened one: "Brahmin_Kashmir" must not land on another region's Brahmins)
    for s in sheets {
        let t = std::fs::read_to_string(s).map_err(|e| format!("{s}: {e}"))?;
        for line in t.lines() {
            // individual sheets ("Pop:ID,...") and population-average sheets ("Pop,...") both count
            let label = line.split(',').next().unwrap_or("").trim();
            let pop = label.rsplit_once(':').map_or(label, |(p, _)| p);
            if pop.is_empty() || acc.contains_key(pop) {
                continue;
            }
            if let Some(&(_, la, lo)) = MANUAL.iter().find(|m| m.0 == pop) {
                acc.insert(pop.to_string(), (la, lo, 1));
                continue;
            }
            if let Some(&(la, lo, n)) = groups.get(&norm_group(pop)) {
                acc.insert(pop.to_string(), (la / n as f64, lo / n as f64, 1));
            }
            if acc.contains_key(pop) || (countries.is_empty() && provinces.is_empty()) {
                continue;
            }
            // last resort: a country named in the label ("Ashkenazi_Poland"), most specific word
            // first; a leading nationality word only when the rest is a code or a direction
            // ("Austrian", "Lithuanian_PA", "German_East") - in "Armenian_Urfa" or
            // "Armenian_Gesaria" it says nothing about where the population lives
            let words: Vec<&str> = pop.split('_').collect();
            let single = words.len() == 1;
            let demonym_ok = words[1..].iter().all(|w| w.len() <= 3 || GENERIC.contains(&fold(w).as_str()));
            for (wi, word) in words.iter().enumerate().rev() {
                let word = *word;
                // "EmiratiA" / "BelgianB": drop a trailing sub-group letter
                let w = word.strip_suffix(|c: char| c.is_ascii_uppercase()).filter(|w| w.chars().last().is_some_and(|c| c.is_lowercase())).unwrap_or(word);
                let w = fold(w);
                if w.len() < 3 || GENERIC.contains(&w.as_str()) {
                    continue;
                }
                // the population's own word is never a province here ("Adygei" the people vs.
                // "Adygey" the republic is fine, but "Georgian" must not match a province)
                if !single {
                    if let Some((la, lo)) = resolve(provinces.get(&w)) {
                        acc.insert(pop.to_string(), (la, lo, 1));
                        break;
                    }
                }
                let country = if countries.contains_key(&w) {
                    Some(w.as_str())
                } else if wi == 0 && demonym_ok {
                    DEMONYMS.iter().find(|d| d.0 == w).map(|d| d.1)
                } else {
                    None
                };
                if let Some(c) = country.filter(|c| !TOO_BIG.contains(c)) {
                    if let Some((la, lo)) = resolve(countries.get(c)) {
                        acc.insert(pop.to_string(), (la, lo, 1));
                        break;
                    }
                }
            }
        }
    }
    let mut w = std::fs::File::create(out).map_err(|e| format!("{out}: {e}"))?;
    let mut pops: Vec<_> = acc.into_iter().collect();
    pops.sort_by(|a, b| a.0.cmp(&b.0));
    for (p, (la, lo, n)) in &pops {
        writeln!(w, "{p}\t{la:.4}\t{lo:.4}\t{n}").map_err(|e| e.to_string())?;
    }
    Ok(pops.len())
}

/// The locations at `path` if that file exists (an optional override), otherwise the built-in ones.
pub fn load_places(path: &str) -> Result<HashMap<String, (f64, f64)>, String> {
    let t = std::fs::read_to_string(path).unwrap_or_else(|_| BUILTIN_PLACES.to_string());
    Ok(t.lines()
        .filter_map(|l| {
            let f: Vec<&str> = l.split('\t').collect();
            Some((f.first()?.to_string(), (f.get(1)?.parse().ok()?, f.get(2)?.parse().ok()?)))
        })
        .collect())
}

// ---------------------------------------------------------------- heat
pub const RAMP: [[u8; 3]; 12] = [
    [163, 45, 45], [226, 75, 74], [216, 90, 48], [239, 130, 40], [239, 159, 39], [250, 199, 117],
    [242, 220, 120], [215, 228, 140], [192, 221, 151], [151, 196, 89], [99, 153, 34], [59, 109, 17],
];

pub fn ramp(t: f64) -> [u8; 3] {
    let t = t.clamp(0.0, 1.0) * (RAMP.len() - 1) as f64;
    let i = (t.floor() as usize).min(RAMP.len() - 2);
    let f = t - i as f64;
    let (a, b) = (RAMP[i], RAMP[i + 1]);
    [0, 1, 2].map(|k| (a[k] as f64 + (b[k] as f64 - a[k] as f64) * f).round() as u8)
}

/// A population on the map with its genetic distance to the user.
pub struct Point {
    pub name: String,
    pub lat: f64,
    pub lon: f64,
    pub dist: f64,
    pub x: f64,
    pub y: f64,
}

pub struct Heat {
    pub points: Vec<Point>, // sorted by distance, closest first
    pub rgba: Vec<u8>,      // premultiplied RGBA raster covering the map
    pub w: usize,
    pub h: usize,
    pub top: f64,    // map-unit y of the raster's first row
    pub bottom: f64, // map-unit y of the last row
}

/// Kept off the heatmap (still used for modeling): groups that live far from where their
/// ancestry comes from (Jewish diaspora groups, Roma, Parsis, Afrikaners, Kalmyks, who moved to
/// the Volga in the 1600s) and samples flagged as outliers ("_o"). Plotted where they were
/// sampled, they break up the map's gradient.
pub fn off_heatmap(name: &str) -> bool {
    let n = name.trim_end_matches(" (est.)").to_lowercase();
    let words: Vec<&str> = n.split(['_', '.', '-', ' ']).filter(|w| !w.is_empty()).collect();
    let outlier = words.iter().skip(1).any(|w| w.starts_with('o') && w[1..].chars().all(|c| c.is_ascii_digit()))
        || words.iter().any(|w| matches!(*w, "outlier" | "outliers"));
    let diaspora = words.iter().any(|w| {
        matches!(
            *w,
            "jew" | "jews" | "jewish" | "ashkenazi" | "ashkenazim" | "sephardi" | "sephardic" | "mizrahi" | "karaite"
                | "roma" | "romani" | "gypsy" | "sinti" | "parsi" | "afrikaner" | "boer" | "kalmyk" | "siddi"
        )
    });
    outlier || diaspora
}

/// Distance from `me` to every placed population of `sheet`, then the IDW raster.
pub fn compute(me: &[f64], sheet: &str, places: &HashMap<String, (f64, f64)>) -> Heat {
    let mut points: Vec<Point> = sheet
        .lines()
        .filter_map(|l| {
            let mut it = l.split(',');
            let name = it.next()?.trim().to_string();
            if off_heatmap(&name) {
                return None;
            }
            let v: Vec<f64> = it.filter_map(|x| x.trim().parse().ok()).collect();
            if v.len() != me.len() {
                return None;
            }
            let &(lat, lon) = places.get(&name)?;
            let d = v.iter().zip(me).map(|(a, b)| (a - b).powi(2)).sum::<f64>().sqrt();
            Some(Point { name, lat, lon, dist: d, x: merc_x(lon), y: merc_y(lat) })
        })
        .collect();
    points.sort_by(|a, b| a.dist.partial_cmp(&b.dist).unwrap_or(std::cmp::Ordering::Equal));
    let dmin = points.first().map(|p| p.dist).unwrap_or(0.0);
    let dmax = points.get(points.len() / 2).map(|p| p.dist).unwrap_or(dmin + 0.1).max(dmin + 1e-6);

    let (w, top, bottom) = (900usize, merc_y(LAT_TOP), merc_y(LAT_BOTTOM));
    let h = ((bottom - top) / MAP_W * w as f64).round() as usize;
    let mut rgba = vec![0u8; w * h * 4];
    let pts: Vec<(f64, f64, f64)> = points.iter().map(|p| (p.lat, p.lon, (p.dist - dmin) / (dmax - dmin))).collect();
    // rows are handed out one at a time so the slower cores don't hold things up
    let next = std::sync::Mutex::new(rgba.chunks_mut(w * 4).enumerate());
    std::thread::scope(|s| {
        for _ in 0..crate::linalg::threads() {
            s.spawn(|| {
                // the latitude term only changes per row
                let mut dy2 = vec![0.0f64; pts.len()];
                let mut wv = vec![0.0f64; pts.len()];
                loop {
                    let job = next.lock().unwrap().next();
                    let Some((ri, row)) = job else { break };
                    let ry = top + (ri as f64 + 0.5) * (bottom - top) / h as f64;
                    let lat = inv_merc_y(ry);
                    let cl = lat.to_radians().cos();
                    for (d, &(plat, _, _)) in dy2.iter_mut().zip(&pts) {
                        let dy = lat - plat;
                        *d = dy * dy;
                    }
                    for x in 0..w {
                        let lon = (x as f64 + 0.5) / w as f64 * 360.0 - 180.0;
                        let mut nearest = f64::MAX;
                        for ((wk, &(_, plon, _)), &dyy) in wv.iter_mut().zip(&pts).zip(&dy2) {
                            let dx = (lon - plon) * cl;
                            let d2 = dx * dx + dyy;
                            nearest = nearest.min(d2);
                            *wk = d2;
                        }
                        // fade out far from any sampled population; fully faded pixels stay
                        // transparent (all zero), so skip the weights there
                        let a = (1.0 - (nearest.sqrt() - 10.0) / 8.0).clamp(0.0, 1.0);
                        if a == 0.0 {
                            continue;
                        }
                        for wk in wv.iter_mut() {
                            let d2 = *wk;
                            *wk = 1.0 / ((d2 + 0.5) * (d2 + 0.5));
                        }
                        // running sums in point order
                        let (mut sw, mut sv) = (0.0, 0.0);
                        for (&wgt, &(_, _, pt)) in wv.iter().zip(&pts) {
                            sw += wgt;
                            sv += wgt * pt;
                        }
                        if sw == 0.0 {
                            continue;
                        }
                        let c = ramp(sv / sw);
                        let o = x * 4;
                        row[o] = (c[0] as f64 * a) as u8;
                        row[o + 1] = (c[1] as f64 * a) as u8;
                        row[o + 2] = (c[2] as f64 * a) as u8;
                        row[o + 3] = (255.0 * a) as u8;
                    }
                }
            });
        }
    });
    Heat { points, rgba, w, h, top, bottom }
}

/// Per-sample location table from annotation files (AADR `.anno` / Poseidon `.janno`):
/// `ID <tab> group <tab> lat <tab> lon <tab> modern(1/0)`. Used to place AADR samples that have
/// no official G25 coordinates (the estimated heatmap layer).
pub fn build_locations(annos: &str, out: &str) -> Result<usize, String> {
    let mut seen = std::collections::HashSet::new();
    let mut w = std::io::BufWriter::new(std::fs::File::create(out).map_err(|e| format!("{out}: {e}"))?);
    let mut n = 0;
    for anno in annos.split(',').map(str::trim).filter(|a| !a.is_empty()) {
        let f = std::fs::File::open(anno).map_err(|e| format!("{anno}: {e}"))?;
        let mut lines = BufReader::new(f).lines().map_while(Result::ok);
        let Some(header) = lines.next() else { continue };
        let cols: Vec<String> = header.split('\t').map(|s| s.trim().to_lowercase()).collect();
        let pos = |names: &[&str]| cols.iter().position(|c| names.contains(&c.as_str()));
        let (Some(la), Some(lo), Some(gr)) = (pos(&["latitude", "lat."]), pos(&["longitude", "long."]), pos(&["group_name", "group id", "group_id"])) else { continue };
        let date = pos(&["date_type"]);
        let mean_bp = cols.iter().position(|c| c.contains("date mean in bp"));
        for l in lines {
            let f: Vec<&str> = l.split('\t').collect();
            let id = f[0].trim();
            let (Some(Ok(lat)), Some(Ok(lon))) = (f.get(la).map(|x| x.trim().parse::<f64>()), f.get(lo).map(|x| x.trim().parse::<f64>())) else { continue };
            let group = f.get(gr).map(|g| g.trim()).unwrap_or("");
            if id.is_empty() || group.is_empty() || !seen.insert(id.to_lowercase()) {
                continue;
            }
            let modern = match (date.and_then(|c| f.get(c)), mean_bp.and_then(|c| f.get(c))) {
                (Some(d), _) => d.trim().eq_ignore_ascii_case("modern"),
                (None, Some(bp)) => bp.trim().parse::<f64>().map(|v| v == 0.0).unwrap_or(false),
                _ => false,
            };
            writeln!(w, "{id}\t{group}\t{lat}\t{lon}\t{}", modern as u8).map_err(|e| e.to_string())?;
            n += 1;
        }
    }
    Ok(n)
}

/// Built-in places plus optional extra ones (the estimated layer); built-in names win.
pub fn load_places_with_extras(path: &str, extra: &str) -> Result<HashMap<String, (f64, f64)>, String> {
    let mut p = load_places(path)?;
    if let Ok(t) = std::fs::read_to_string(extra) {
        for l in t.lines() {
            let f: Vec<&str> = l.split('\t').collect();
            if let (Some(n), Some(Ok(la)), Some(Ok(lo))) = (f.first(), f.get(1).map(|x| x.parse()), f.get(2).map(|x| x.parse())) {
                p.entry(n.to_string()).or_insert((la, lo));
            }
        }
    }
    for (name, fix) in PLACE_FIXES {
        match fix {
            Some(at) if p.contains_key(*name) => {
                p.insert(name.to_string(), *at);
            }
            None => {
                p.remove(*name);
            }
            _ => {}
        }
    }
    Ok(p)
}

/// Mistakes in the AADR location annotations, fixed by hand (lat, lon; None = no dot).
/// - Kurd_WGA: its two samples have Algerian IDs (ALG_LN_203/204) and Algerian coordinates,
///   while the Kurdish KRD_010/011 samples are labelled Ain_Touta, a town in Algeria. The labels
///   got swapped. The genotypes are Kurdish (G25 right next to Kurd_Iraq), so it belongs where the
///   KRD samples are, which is within 250 km of official Kurd dots, so it gets no estimated dot.
/// - Kurd_USSR: latitude and longitude are swapped in AADR (44.58, 39.42 is in Adygea).
pub const PLACE_FIXES: &[(&str, Option<(f64, f64)>)] = &[
    ("Kurd_WGA (est.)", None),
    ("Kurd_USSR", Some((39.42, 44.58))),
];

/// AADR group labels whose annotated coordinates are wrong, with the right ones (lat, lon).
pub const AADR_GROUP_FIXES: &[(&str, (f64, f64))] = &[("Kurd_WGA", (40.7, 44.3)), ("Ain_Touta", (35.4, 5.9)), ("Kurd", (39.42, 44.58))];
