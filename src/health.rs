//! Health info: a curated set of well-studied SNPs read straight from the raw file.
//! Genotypes are read on the forward strand, as 23andMe, AncestryDNA, MyHeritage and FTDNA export them.
//! Common variants nudge the odds; they aren't diagnoses, and chips miss rare mutations.

use crate::raw::{RawGenotypes, X_FLAG};

#[derive(Clone, Copy, PartialEq)]
enum Kind {
    Trait,
    Risk,
    Drug,
    Fun,
}

struct Rule {
    rsid: u32,
    gene: &'static str,
    topic: &'static str,
    allele: u8, // the allele that is counted (risk / effect allele)
    kind: Kind,
    text: [&'static str; 3], // meaning for 0, 1, 2 copies
}

const fn r(rsid: u32, gene: &'static str, topic: &'static str, allele: u8, kind: Kind, text: [&'static str; 3]) -> Rule {
    Rule { rsid, gene, topic, allele, kind, text }
}

const RULES: &[Rule] = &[
    // traits
    r(12913832, "HERC2", "Eye color", b'G', Kind::Trait, ["brown eyes very likely", "brown, hazel or green likely", "blue eyes likely"]),
    r(1426654, "SLC24A5", "Skin pigmentation", b'A', Kind::Trait, ["no copies of the West Eurasian light-skin variant", "one copy of the light-skin variant", "two copies of the light-skin variant"]),
    r(12821256, "KITLG", "Blond hair variant", b'C', Kind::Trait, ["no copies of the European blond variant", "one copy of the blond variant", "two copies of the blond variant"]),
    r(3827760, "EDAR", "EDAR 370A (hair thickness, teeth)", b'G', Kind::Trait, ["typical West Eurasian/African type", "one copy: linked to thicker, straighter hair", "two copies: thick straight hair, shovel incisors common"]),
    r(17822931, "ABCC11", "Earwax", b'T', Kind::Trait, ["wet earwax", "wet earwax", "dry earwax"]),
    r(1815739, "ACTN3", "Muscle fiber type", b'T', Kind::Trait, ["RR: sprint/power-type muscle", "RX: mixed", "XX: endurance-leaning, no alpha-actinin-3"]),
    // risks (allele = risk allele)
    r(1061170, "CFH", "Macular degeneration", b'C', Kind::Risk, ["typical", "one risk copy (Y402H)", "two risk copies (Y402H): notably higher"]),
    r(10490924, "ARMS2", "Macular degeneration", b'T', Kind::Risk, ["typical", "one risk copy", "two risk copies: notably higher"]),
    r(10757278, "9p21", "Coronary heart disease", b'G', Kind::Risk, ["lower than average", "average (about half of people)", "higher (~1.3–1.6×)"]),
    r(9939609, "FTO", "Obesity tendency", b'A', Kind::Risk, ["typical", "slightly higher", "higher (~+3 kg on average; offset by activity)"]),
    r(7903146, "TCF7L2", "Type 2 diabetes", b'T', Kind::Risk, ["lowest-risk genotype", "slightly higher", "higher (~1.8×)"]),
    r(6025, "F5", "Blood clots (Factor V Leiden)", b'T', Kind::Risk, ["no Factor V Leiden", "carrier: higher clot risk", "two copies: much higher clot risk"]),
    r(1799963, "F2", "Blood clots (prothrombin)", b'A', Kind::Risk, ["no prothrombin mutation", "carrier: higher clot risk", "two copies: much higher clot risk"]),
    r(1800562, "HFE C282Y", "Hemochromatosis", b'A', Kind::Risk, ["no C282Y", "C282Y carrier", "two copies of C282Y: high risk"]),
    r(1799945, "HFE H63D", "Hemochromatosis", b'G', Kind::Risk, ["no H63D", "H63D carrier (common, harmless alone)", "two copies of H63D: mild"]),
    r(28929474, "SERPINA1 Z", "Alpha-1 antitrypsin", b'T', Kind::Risk, ["no Z allele", "Z carrier", "ZZ: deficiency"]),
    r(1050828, "G6PD A-", "G6PD deficiency", b'T', Kind::Risk, ["no A- variant", "A- variant present", "A- variant present"]),
    r(5030868, "G6PD Med", "G6PD deficiency", b'A', Kind::Risk, ["no Mediterranean variant", "Mediterranean variant present", "Mediterranean variant present"]),
    r(4349859, "HLA-B27 tag", "Ankylosing spondylitis", b'A', Kind::Risk, ["B27 negative: low", "B27 likely: higher", "B27 likely: higher"]),
    r(10484554, "HLA-C*06 tag", "Psoriasis", b'T', Kind::Risk, ["typical", "one copy: higher", "two copies: notably higher"]),
    r(2200733, "PITX2", "Atrial fibrillation", b'T', Kind::Risk, ["typical", "one copy: ~1.4×", "two copies: higher"]),
    r(16969968, "CHRNA5", "Nicotine dependence (if smoking)", b'A', Kind::Risk, ["typical", "slightly higher", "higher: heavier smoking, harder quitting"]),
    r(3923809, "BTBD9", "Restless legs", b'A', Kind::Risk, ["lower", "average", "higher"]),
    r(738409, "PNPLA3", "Fatty liver", b'G', Kind::Risk, ["typical", "one I148M copy: modestly higher", "two I148M copies: higher"]),
    r(10455872, "LPA", "High Lp(a)", b'G', Kind::Risk, ["no high-Lp(a) variant", "carrier: higher Lp(a)", "two copies: high Lp(a)"]),
    r(2231142, "ABCG2", "Gout", b'T', Kind::Risk, ["typical", "slightly higher", "notably higher"]),
    r(2476601, "PTPN22", "Autoimmune (RA, lupus, T1D)", b'A', Kind::Risk, ["typical", "higher", "notably higher"]),
    r(3135388, "HLA-DRB1*15 tag", "Multiple sclerosis", b'A', Kind::Risk, ["typical", "higher", "notably higher"]),
    // drug response
    r(1799853, "CYP2C9*2", "Warfarin / NSAIDs", b'T', Kind::Drug, ["normal", "*2 carrier: slower metabolism", "*2/*2: slow metabolism"]),
    r(1057910, "CYP2C9*3", "Warfarin / NSAIDs", b'C', Kind::Drug, ["normal", "*3 carrier: slower metabolism", "*3/*3: slow metabolism"]),
    r(9923231, "VKORC1", "Warfarin sensitivity", b'T', Kind::Drug, ["normal dose", "more sensitive", "much more sensitive"]),
    r(4244285, "CYP2C19*2", "Clopidogrel, some antidepressants", b'A', Kind::Drug, ["normal", "intermediate metabolizer", "poor metabolizer"]),
    r(12248560, "CYP2C19*17", "Clopidogrel, some antidepressants", b'T', Kind::Drug, ["normal", "rapid metabolizer", "ultrarapid metabolizer"]),
    r(4149056, "SLCO1B1", "Statin muscle side effects", b'C', Kind::Drug, ["typical", "higher risk on simvastatin", "high risk on simvastatin"]),
    r(1801133, "MTHFR C677T", "Folate metabolism", b'A', Kind::Drug, ["normal", "slightly reduced", "reduced (~30% activity)"]),
    // fun
    r(762551, "CYP1A2", "Caffeine", b'C', Kind::Fun, ["fast metabolizer", "slow metabolizer", "slow metabolizer"]),
    r(671, "ALDH2", "Alcohol flush", b'A', Kind::Fun, ["no flush", "flushes", "strong flush, poor tolerance"]),
    r(1229984, "ADH1B", "Alcohol metabolism", b'T', Kind::Fun, ["standard", "fast (unpleasant when drinking)", "very fast"]),
    r(4481887, "OR2M7", "Smelling asparagus pee", b'A', Kind::Fun, ["probably can't smell it", "may smell it", "likely smells it"]),
    r(10427255, "ZEB2", "Photic sneeze (sun sneezing)", b'C', Kind::Fun, ["unlikely", "possible", "more likely"]),
];

fn gt(g: &RawGenotypes, rs: u32) -> Option<[u8; 2]> {
    g.get(&rs).or_else(|| g.get(&(rs | X_FLAG))).copied()
}

fn show(g: [u8; 2]) -> String {
    let mut v = [g[0], g[1]];
    v.sort();
    String::from_utf8_lossy(&v).into_owned()
}

fn apoe(g: &RawGenotypes) -> Option<String> {
    let (a, b) = (gt(g, 429358)?, gt(g, 7412)?);
    let e4 = a.iter().filter(|&&x| x == b'C').count();
    let e2 = b.iter().filter(|&&x| x == b'T').count();
    let mut alleles = vec![];
    for _ in 0..e4 { alleles.push("ε4"); }
    for _ in 0..e2 { alleles.push("ε2"); }
    while alleles.len() < 2 { alleles.push("ε3"); }
    alleles.sort();
    let t = alleles.join("/");
    let note = match (e4, e2) {
        (0, 0) => "the most common type, average Alzheimer's risk",
        (0, _) => "ε2 lowers Alzheimer's risk",
        (1, _) => "one ε4: higher Alzheimer's risk (~3×)",
        _ => "two ε4: much higher Alzheimer's risk",
    };
    Some(format!("APOE {t}: {note}"))
}

fn taster(g: &RawGenotypes) -> Option<String> {
    let a = gt(g, 713598)?;
    let b = gt(g, 10246939)?;
    let pav = a.iter().filter(|&&x| x == b'G').count().min(b.iter().filter(|&&x| x == b'C').count());
    Some(match pav {
        2 => "TAS2R38 PAV/PAV: strong bitter taster (broccoli, coffee, IPAs taste harsher)".into(),
        1 => "TAS2R38 one PAV copy: medium bitter taster".into(),
        _ => "TAS2R38 AVI/AVI: can't taste PTC-type bitterness".into(),
    })
}

fn celiac(g: &RawGenotypes) -> Option<String> {
    let dq25 = gt(g, 2187668).map(|x| x.contains(&b'T'));
    let dq8 = gt(g, 7454108).map(|x| x.contains(&b'C'));
    match (dq25, dq8) {
        (None, None) => None,
        (Some(false) | None, Some(false) | None) => Some("Celiac / type 1 diabetes: no DQ2.5 or DQ8 tags — low".into()),
        _ => Some("Celiac / type 1 diabetes: carries a DQ2.5 or DQ8 tag — higher (most carriers never develop celiac)".into()),
    }
}

fn lactase(g: &RawGenotypes) -> Option<String> {
    if let Some(x) = gt(g, 4988235) {
        let n = x.iter().filter(|&&a| a == b'A').count();
        return Some(match n { 0 => "Lactose: no European persistence allele — likely intolerant as an adult", 1 => "Lactose: one persistence allele — likely tolerant", _ => "Lactose: two persistence alleles — tolerant" }.into());
    }
    let x = gt(g, 182549)?;
    let n = x.iter().filter(|&&a| a == b'T').count();
    Some(match n { 0 => "Lactose: no European persistence allele (proxy SNP) — likely intolerant", _ => "Lactose: persistence allele present (proxy SNP) — likely tolerant" }.into())
}

/// Plain-text report, grouped into sections.
pub fn report(g: &RawGenotypes) -> String {
    let mut traits = vec![];
    let (mut higher, mut average, mut lower) = (vec![], vec![], vec![]);
    let mut drugs = vec![];
    let mut fun = vec![];
    let mut missing = 0;

    for rule in RULES {
        let Some(x) = gt(g, rule.rsid) else {
            missing += 1;
            continue;
        };
        let n = x.iter().filter(|&&a| a == rule.allele).count();
        let line = format!("{} — {} (rs{} {}): {}", rule.topic, rule.gene, rule.rsid, show(x), rule.text[n]);
        match rule.kind {
            Kind::Trait => traits.push(line),
            Kind::Drug => drugs.push(line),
            Kind::Fun => fun.push(line),
            Kind::Risk => match n {
                2 => higher.push(line),
                1 => average.push(line),
                _ => lower.push(line),
            },
        }
    }
    if let Some(s) = apoe(g) {
        let line = format!("Alzheimer's — {s}");
        if s.contains("two ε4") { higher.insert(0, line) } else if s.contains("one ε4") { average.insert(0, line) } else { lower.insert(0, line) }
    }
    if let Some(s) = celiac(g) {
        if s.contains("low") { lower.push(s) } else { average.push(s) }
    }
    if let Some(s) = lactase(g) { fun.push(s); }
    if let Some(s) = taster(g) { fun.push(s); }

    let mut out = String::new();
    let mut section = |title: &str, items: &[String]| {
        if items.is_empty() { return; }
        out.push_str(&format!("{title}\n"));
        for i in items {
            out.push_str(&format!("  • {i}\n"));
        }
        out.push('\n');
    };
    section("TRAITS", &traits);
    section("HIGHER THAN AVERAGE", &higher);
    section("ONE RISK COPY / AVERAGE", &average);
    section("TYPICAL OR LOWER", &lower);
    section("DRUG RESPONSE", &drugs);
    section("FUN FACTS", &fun);
    out.push_str(&format!(
        "{} of {} markers were not on your chip.\nThese are common variants that nudge odds — not diagnoses. Consumer chips miss most rare serious mutations (BRCA, Lynch, cystic fibrosis).",
        missing, RULES.len()
    ));
    out
}

// ================================================================ cards for the Health page
// Same layout as Physical characteristics: one card per item, with an icon, the finding, and
// the DNA behind it. Risks are ranked most alarming first; medicine notes next; fun facts last.

/// Which cartoon icon a card shows.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Icon {
    Heart,
    HeartRhythm,
    Eye,
    Brain,
    Clot,
    Iron,
    Liver,
    Lungs,
    Scale,
    Sugar,
    Spine,
    Skin,
    Foot,
    Legs,
    Shield,
    Nerve,
    Cigarette,
    Pill,
    Coffee,
    Drink,
    Asparagus,
    Sun,
    Milk,
    Wheat,
    Broccoli,
    Blood,
}

/// How a finding compares with the average person.
#[derive(Clone, Copy, PartialEq, PartialOrd, Debug)]
pub enum Level {
    Lower,
    Typical,
    Raised,
    Higher,
    Note, // medicine response
    Fun,
}
impl Level {
    pub fn label(self) -> &'static str {
        match self {
            Level::Higher => "Higher than average",
            Level::Raised => "Slightly raised",
            Level::Typical => "Average",
            Level::Lower => "Lower than average",
            Level::Note => "Tell your doctor",
            Level::Fun => "Fun fact",
        }
    }
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Section {
    Risk,
    Medicine,
    Fun,
}

#[derive(Clone, Debug)]
pub struct Item {
    pub section: Section,
    pub icon: Icon,
    pub title: String,
    pub value: String,
    /// the DNA behind it: gene, marker and your genotype
    pub dna: String,
    pub level: Level,
    /// higher = more alarming (only used to rank risks)
    pub score: u32,
}

/// icon, seriousness 1-3, and whether one copy already matters (dominant-acting variants)
fn meta(rs: u32) -> (Icon, u32, bool) {
    match rs {
        1061170 | 10490924 => (Icon::Eye, 2, false),
        10757278 => (Icon::Heart, 3, false),
        9939609 => (Icon::Scale, 1, false),
        7903146 => (Icon::Sugar, 2, false),
        6025 | 1799963 => (Icon::Clot, 3, true),
        1800562 => (Icon::Iron, 2, false),
        1799945 => (Icon::Iron, 1, false),
        28929474 => (Icon::Lungs, 2, false),
        1050828 | 5030868 => (Icon::Blood, 2, true),
        4349859 => (Icon::Spine, 2, true),
        10484554 => (Icon::Skin, 1, false),
        2200733 => (Icon::HeartRhythm, 2, false),
        16969968 => (Icon::Cigarette, 1, false),
        3923809 => (Icon::Legs, 1, false),
        738409 => (Icon::Liver, 2, false),
        10455872 => (Icon::Heart, 2, true),
        2231142 => (Icon::Foot, 1, false),
        2476601 => (Icon::Shield, 2, false),
        3135388 => (Icon::Nerve, 2, false),
        762551 => (Icon::Coffee, 0, false),
        671 | 1229984 => (Icon::Drink, 0, false),
        4481887 => (Icon::Asparagus, 0, false),
        10427255 => (Icon::Sun, 0, false),
        _ => (Icon::Pill, 1, false),
    }
}

fn cap(s: &str) -> String {
    let mut c = s.chars();
    c.next().map(|f| f.to_uppercase().collect::<String>() + c.as_str()).unwrap_or_default()
}

/// The Health page's cards, ranked. Physical traits (eye colour etc.) live on their own page.
pub fn items(g: &RawGenotypes) -> Vec<Item> {
    let mut risks = vec![];
    let mut meds = vec![];
    let mut fun = vec![];
    for rule in RULES {
        if rule.kind == Kind::Trait {
            continue;
        }
        let Some(x) = gt(g, rule.rsid) else { continue };
        let n = x.iter().filter(|&&a| a == rule.allele).count();
        let (icon, weight, dominant) = meta(rule.rsid);
        let dna = format!("{} · rs{} {} · {} cop{} of the {} allele", rule.gene, rule.rsid, show(x), n, if n == 1 { "y" } else { "ies" }, rule.allele as char);
        let mut item = Item { section: Section::Risk, icon, title: rule.topic.to_string(), value: cap(rule.text[n]), dna, level: Level::Typical, score: 0 };
        match rule.kind {
            Kind::Risk => {
                // 9p21: no copies is genuinely lower, one copy is the population average
                item.level = match (n, dominant, rule.rsid) {
                    (0, _, 10757278) | (0, _, 3923809) => Level::Lower,
                    (1, _, 10757278) | (1, _, 3923809) => Level::Typical,
                    // H63D alone is common and harmless
                    (1, _, 1799945) => Level::Typical,
                    (0, _, _) => Level::Typical,
                    (_, true, _) | (2, _, _) => Level::Higher,
                    _ => Level::Raised,
                };
                item.score = match item.level { Level::Higher => 20, Level::Raised => 10, Level::Typical => 5, _ => 0 } + weight;
                risks.push(item);
            }
            Kind::Drug => {
                item.section = Section::Medicine;
                item.level = if n == 0 { Level::Typical } else { Level::Note };
                item.score = n as u32;
                meds.push(item);
            }
            _ => {
                item.section = Section::Fun;
                item.level = Level::Fun;
                fun.push(item);
            }
        }
    }
    // APOE (two markers) and the HLA celiac tags
    if let (Some(a), Some(b)) = (gt(g, 429358), gt(g, 7412)) {
        let e4 = a.iter().filter(|&&x| x == b'C').count();
        let s = apoe(g).unwrap_or_default();
        let level = match e4 { 2 => Level::Higher, 1 => Level::Raised, _ => if s.contains("ε2") { Level::Lower } else { Level::Typical } };
        let value = s.split_once(": ").map(|(_, v)| cap(v)).unwrap_or_default();
        let type_ = s.split_once(':').map(|(t, _)| t.to_string()).unwrap_or_default();
        risks.push(Item {
            section: Section::Risk, icon: Icon::Brain, title: "Alzheimer's disease".into(), value,
            dna: format!("{type_} · rs429358 {} + rs7412 {}", show(a), show(b)), level,
            score: match level { Level::Higher => 23, Level::Raised => 13, Level::Typical => 5, _ => 0 },
        });
    }
    if let Some(s) = celiac(g) {
        let low = s.contains("— low");
        risks.push(Item {
            section: Section::Risk, icon: Icon::Wheat, title: "Celiac disease / type 1 diabetes".into(),
            value: cap(s.split_once(": ").map(|x| x.1).unwrap_or(&s)),
            dna: format!("HLA-DQ2.5 / DQ8 tags · rs2187668 {} · rs7454108 {}",
                gt(g, 2187668).map(show).unwrap_or("–".into()), gt(g, 7454108).map(show).unwrap_or("–".into())),
            level: if low { Level::Lower } else { Level::Raised }, score: if low { 0 } else { 11 },
        });
    }
    if let Some(s) = lactase(g) {
        let (rs, x) = if let Some(x) = gt(g, 4988235) { (4988235, x) } else { (182549, gt(g, 182549).unwrap_or(*b"--")) };
        fun.insert(0, Item {
            section: Section::Fun, icon: Icon::Milk, title: "Milk as an adult".into(),
            value: cap(s.split_once(": ").map(|x| x.1).unwrap_or(&s)),
            dna: format!("MCM6 / lactase persistence · rs{rs} {}", show(x)), level: Level::Fun, score: 0,
        });
    }
    if let Some(s) = taster(g) {
        let (gene, rest) = s.split_once(": ").unwrap_or(("TAS2R38", &s));
        fun.push(Item {
            section: Section::Fun, icon: Icon::Broccoli, title: "Bitter taste".into(), value: cap(rest),
            dna: format!("{gene} · rs713598 {} · rs10246939 {}", gt(g, 713598).map(show).unwrap_or("–".into()), gt(g, 10246939).map(show).unwrap_or("–".into())),
            level: Level::Fun, score: 0,
        });
    }
    // most alarming first; ties keep the curated order
    risks.sort_by(|a, b| b.score.cmp(&a.score));
    meds.sort_by(|a, b| b.score.cmp(&a.score));
    risks.into_iter().chain(meds).chain(fun).collect()
}
