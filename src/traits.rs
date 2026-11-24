//! Physical characteristics from a raw DNA file.
//!
//! Predictions are simplified additive scores over well-replicated pigmentation and appearance
//! SNPs (HERC2/OCA2, SLC24A5, SLC45A2, TYR, IRF4, KITLG, SLC24A4, MC1R, EDAR, ACTN3), read on the
//! forward strand. A marker missing from the chip counts as "neutral" (half its maximum effect).
//! Every trait carries a confidence level so the UI can say how much to trust it.

use crate::raw::{RawGenotypes, X_FLAG};

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Confidence {
    High,
    Medium,
    Low,
}
impl Confidence {
    pub fn label(self) -> &'static str {
        match self {
            Confidence::High => "High confidence",
            Confidence::Medium => "Medium confidence",
            Confidence::Low => "Low confidence",
        }
    }
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Sex {
    Male,
    Female,
    Unknown,
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Texture {
    Straight,
    Wavy,
    Curly,
}

/// What each card shows; `kind` tells the UI which icon to draw.
#[derive(Clone, Debug)]
pub struct Trait {
    pub kind: Kind,
    pub title: &'static str,
    pub value: String,
    pub detail: String,
    pub confidence: Confidence,
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Kind {
    Sex,
    Hairline,
    Earwax,
    Eyes,
    HairColor,
    HairTexture,
    HairThickness,
    Skin,
    Freckles,
    Muscle,
}

#[derive(Clone, Debug)]
pub struct Traits {
    pub sex: Sex,
    pub eye_rgb: [u8; 3],
    pub hair_rgb: [u8; 3],
    pub skin_rgb: [u8; 3],
    pub skin_index: usize, // 0 = darkest ... 7 = lightest
    pub texture: Texture,
    pub thickness: u8, // 0 typical, 1 thicker, 2 thick
    pub freckles: u8,  // 0 few, 1 some, 2 many
    pub balding: u8,   // 0 lower, 1 average, 2 raised
    pub list: Vec<Trait>,
}

pub const SKIN_TONES: [([u8; 3], &str); 8] = [
    ([78, 49, 34], "Dark brown"),
    ([126, 80, 52], "Brown"),
    ([160, 109, 71], "Tan"),
    ([192, 138, 98], "Medium / olive"),
    ([209, 159, 122], "Light-medium"),
    ([227, 176, 143], "Light"),
    ([238, 196, 167], "Fair"),
    ([246, 215, 195], "Very fair"),
];

/// copies of `allele` at rsid, or None if not on the chip
fn count(g: &RawGenotypes, rs: u32, allele: u8) -> Option<f64> {
    g.get(&rs).or_else(|| g.get(&(rs | X_FLAG))).map(|x| x.iter().filter(|&&a| a == allele).count() as f64)
}
/// weighted contribution; a missing marker counts as one copy (neutral midpoint)
fn add(g: &RawGenotypes, rs: u32, allele: u8, w: f64, found: &mut usize) -> f64 {
    match count(g, rs, allele) {
        Some(n) => {
            *found += 1;
            n * w
        }
        None => w,
    }
}
fn gt_text(g: &RawGenotypes, rs: u32) -> String {
    match g.get(&rs).or_else(|| g.get(&(rs | X_FLAG))) {
        Some(x) if x[0] == x[1] && g.contains_key(&(rs | X_FLAG)) => format!("rs{rs} {} (X)", x[0] as char),
        Some(x) => {
            let mut v = [x[0], x[1]];
            v.sort();
            format!("rs{rs} {}", String::from_utf8_lossy(&v))
        }
        None => format!("rs{rs} not on chip"),
    }
}

pub fn infer_sex(g: &RawGenotypes) -> Sex {
    let (mut total, mut het) = (0usize, 0usize);
    for (k, v) in g.iter() {
        if k & X_FLAG != 0 {
            total += 1;
            if v[0] != v[1] {
                het += 1;
            }
        }
    }
    if total < 500 {
        return Sex::Unknown;
    }
    if (het as f64) / (total as f64) < 0.01 { Sex::Male } else { Sex::Female }
}

pub fn predict(g: &RawGenotypes) -> Traits {
    let mut list = vec![];
    let sex = infer_sex(g);
    list.push(Trait {
        kind: Kind::Sex,
        title: "Chromosomal sex",
        value: match sex {
            Sex::Male => "Male (XY)".into(),
            Sex::Female => "Female (XX)".into(),
            Sex::Unknown => "Unknown".into(),
        },
        detail: match sex {
            Sex::Unknown => "Your file has too few X-chromosome markers to tell.".into(),
            _ => "Read from how varied your X-chromosome markers are.".into(),
        },
        confidence: if sex == Sex::Unknown { Confidence::Low } else { Confidence::High },
    });

    // ---- earwax: ABCC11 rs17822931 decides it on its own (C = wet, dominant; TT = dry)
    let dry = count(g, 17822931, b'T');
    list.push(Trait {
        kind: Kind::Earwax,
        title: "Earwax type",
        value: match dry { Some(n) if n >= 2.0 => "Dry (flaky, grey)", Some(_) => "Wet (sticky, honey-coloured)", None => "Unknown (marker not on chip)" }.into(),
        detail: format!("ABCC11 {}: two T copies give dry earwax (common in East Asia); any C gives wet (most of the world).", gt_text(g, 17822931)),
        confidence: if dry.is_some() { Confidence::High } else { Confidence::Low },
    });

    // ---- hairline: male-pattern baldness. The two classic loci: AR/EDA2R on the X
    // (rs6625163 A, OR ~3.3 in the first GWAS) and 20p11.22 (rs1160312 A, OR ~1.6);
    // men carrying both have ~7x the odds. Hundreds of minor loci are not modelled.
    let ar = count(g, 6625163, b'A');
    let p20 = count(g, 1160312, b'A');
    let ar_risk = ar.map(|n| n >= 1.0);
    let p20_n = p20.unwrap_or(0.0);
    let (balding, value, conf): (u8, &str, Confidence) = match (sex, ar_risk) {
        (Sex::Female, _) => (1, "Typical — female thinning is harder to predict", Confidence::Low),
        (_, None) => (1, "Average (AR marker not on chip)", Confidence::Low),
        (_, Some(true)) if p20_n >= 1.0 => (2, "Likely to recede / thin with age", Confidence::Medium),
        (_, Some(true)) => (1, "Average — some thinning with age is typical", Confidence::Medium),
        (_, Some(false)) if p20_n >= 1.0 => (1, "Average", Confidence::Medium),
        (_, Some(false)) => (0, "Likely to keep a fuller hairline", Confidence::Medium),
    };
    list.push(Trait {
        kind: Kind::Hairline,
        title: "Hairline with age",
        value: value.into(),
        detail: format!(
            "{} (androgen receptor: {}) · {} (chr 20: {}). Both risk types together ≈ 7× the odds.",
            gt_text(g, 6625163),
            match ar_risk { Some(true) => "risk type", Some(false) => "protective type", None => "n/a" },
            gt_text(g, 1160312),
            match p20 { Some(n) if n >= 2.0 => "two risk copies", Some(n) if n >= 1.0 => "one risk copy", Some(_) => "no risk copies", None => "n/a" }
        ),
        confidence: conf,
    });

    // ---- eyes (HERC2 rs12913832 dominates; modifiers shift within a class)
    let mut found = 0;
    let herc = count(g, 12913832, b'G');
    let modifiers = add(g, 1800407, b'A', 1.0, &mut found)   // OCA2 R419Q -> green/hazel
        + add(g, 12896399, b'T', 0.5, &mut found)          // SLC24A4 -> lighter
        + add(g, 1393350, b'A', 0.5, &mut found)           // TYR -> lighter
        + add(g, 12203592, b'T', 0.5, &mut found); // IRF4 -> lighter
    let (eye, eye_rgb, eye_conf) = match herc {
        Some(n) if n == 0.0 => {
            if modifiers >= 3.5 { ("Light brown / hazel", [142, 107, 58], Confidence::Medium) } else { ("Brown", [94, 58, 32], Confidence::High) }
        }
        Some(n) if n == 1.0 => {
            if modifiers >= 3.0 { ("Green / hazel", [102, 128, 70], Confidence::Medium) } else { ("Hazel / light brown", [138, 104, 56], Confidence::Medium) }
        }
        Some(_) => {
            if count(g, 1800407, b'A').unwrap_or(0.0) >= 1.0 { ("Blue-green", [94, 140, 140], Confidence::Medium) } else { ("Blue", [79, 134, 198], Confidence::High) }
        }
        None => ("Unknown", [120, 100, 80], Confidence::Low),
    };
    list.push(Trait {
        kind: Kind::Eyes,
        title: "Eye color",
        value: eye.into(),
        detail: format!("{} (main eye-color switch) · {} · {}", gt_text(g, 12913832), gt_text(g, 12896399), gt_text(g, 1393350)),
        confidence: eye_conf,
    });

    // ---- red hair (MC1R)
    let mc1r: f64 = [(1805007, b'T'), (1805008, b'T'), (1805009, b'C'), (1805006, b'A'), (11547464, b'A')]
        .iter()
        .map(|&(rs, a)| count(g, rs, a).unwrap_or(0.0))
        .sum();

    // ---- hair lightness
    let mut found = 0;
    let light = add(g, 12821256, b'C', 1.0, &mut found)      // KITLG blond
        + add(g, 12913832, b'G', 0.7, &mut found)            // HERC2
        + add(g, 16891982, b'G', 0.5, &mut found)            // SLC45A2
        + add(g, 1426654, b'A', 0.3, &mut found)             // SLC24A5
        + add(g, 12203592, b'T', 0.7, &mut found)            // IRF4
        + add(g, 12896399, b'T', 0.5, &mut found)            // SLC24A4
        + add(g, 1393350, b'A', 0.4, &mut found)             // TYR
        - count(g, 3827760, b'G').unwrap_or(0.0) * 1.0; // EDAR 370A -> darker, thicker
    let (hair, mut hair_rgb) = if light < 2.0 {
        ("Black", [31, 26, 23])
    } else if light < 3.5 {
        ("Dark brown", [59, 42, 32])
    } else if light < 5.0 {
        ("Brown", [90, 58, 36])
    } else if light < 6.5 {
        ("Light brown / dark blond", [138, 97, 66])
    } else {
        ("Blond", [201, 164, 92])
    };
    let mut hair_value = hair.to_string();
    if mc1r >= 2.0 {
        hair_value = "Red".into();
        hair_rgb = [166, 69, 42];
    } else if mc1r >= 1.0 {
        hair_value = format!("{hair} with red tints (e.g. in a beard)");
    }
    if count(g, 12821256, b'C').unwrap_or(0.0) >= 1.0 && light < 5.0 && mc1r < 2.0 {
        hair_value.push_str(" — possibly lighter as a child");
    }
    list.push(Trait {
        kind: Kind::HairColor,
        title: "Hair color",
        value: hair_value,
        detail: format!("{} (blond) · {} · MC1R red variants: {}", gt_text(g, 12821256), gt_text(g, 12203592), mc1r as u8),
        confidence: if mc1r >= 2.0 { Confidence::High } else { Confidence::Medium },
    });

    // ---- texture / thickness (EDAR 370A; curl genes like TCHH aren't reliable on chips)
    let edar = count(g, 3827760, b'G');
    let (texture, tex_value, tex_conf) = match edar {
        Some(n) if n >= 2.0 => (Texture::Straight, "Straight", Confidence::High),
        Some(n) if n >= 1.0 => (Texture::Straight, "Straight to slightly wavy", Confidence::Medium),
        Some(_) => (Texture::Wavy, "Wavy to straight (no strong markers)", Confidence::Low),
        None => (Texture::Wavy, "Unknown", Confidence::Low),
    };
    list.push(Trait {
        kind: Kind::HairTexture,
        title: "Hair texture",
        value: tex_value.into(),
        detail: format!("EDAR {} — the 370A variant makes hair straighter and thicker. Curl genes (TCHH) can't be read reliably from chips.", gt_text(g, 3827760)),
        confidence: tex_conf,
    });
    let thickness = edar.unwrap_or(0.0) as u8;
    list.push(Trait {
        kind: Kind::HairThickness,
        title: "Hair thickness",
        value: ["Typical", "Thicker than typical", "Thick"][thickness.min(2) as usize].into(),
        detail: format!("EDAR {}: each 370A copy thickens individual hairs.", gt_text(g, 3827760)),
        confidence: if edar.is_some() { Confidence::Medium } else { Confidence::Low },
    });

    // ---- skin
    let mut found = 0;
    let s = add(g, 1426654, b'A', 1.5, &mut found)   // SLC24A5
        + add(g, 16891982, b'G', 1.0, &mut found)    // SLC45A2
        + add(g, 1042602, b'A', 0.5, &mut found)     // TYR S192Y
        + add(g, 12913832, b'G', 0.5, &mut found)    // HERC2
        + add(g, 12203592, b'T', 0.5, &mut found)    // IRF4
        + count(g, 1800414, b'G').unwrap_or(0.0) * 0.7 // OCA2 East Asian light
        + mc1r * 0.5;
    let skin_index = match s {
        x if x < 1.0 => 0,
        x if x < 2.0 => 1,
        x if x < 3.0 => 2,
        x if x < 4.0 => 3,
        x if x < 5.5 => 4,
        x if x < 7.0 => 5,
        x if x < 8.5 => 6,
        _ => 7,
    };
    let (skin_rgb, skin_name) = SKIN_TONES[skin_index];
    list.push(Trait {
        kind: Kind::Skin,
        title: "Skin tone",
        value: skin_name.into(),
        detail: format!("{} · {} · {} — {found} of 5 main skin markers on your chip", gt_text(g, 1426654), gt_text(g, 1042602), gt_text(g, 16891982)),
        confidence: if found >= 4 { Confidence::Medium } else { Confidence::Low },
    });

    // ---- freckles
    let f = count(g, 12203592, b'T').unwrap_or(0.0) * 1.5 + mc1r * 1.5 + count(g, 12913832, b'G').unwrap_or(0.0) * 0.3;
    let freckles = if f < 1.0 { 0 } else if f < 2.5 { 1 } else { 2 };
    list.push(Trait {
        kind: Kind::Freckles,
        title: "Freckles",
        value: ["Few or none", "Some (more after sun)", "Many"][freckles as usize].into(),
        detail: format!("IRF4 {} · MC1R red variants: {}", gt_text(g, 12203592), mc1r as u8),
        confidence: Confidence::Medium,
    });

    // ---- muscle fiber type
    let actn3 = count(g, 1815739, b'T');
    list.push(Trait {
        kind: Kind::Muscle,
        title: "Muscle type",
        value: match actn3 {
            Some(n) if n >= 2.0 => "Endurance-leaning (ACTN3 XX)",
            Some(n) if n >= 1.0 => "Mixed (ACTN3 RX)",
            Some(_) => "Power / sprint-leaning (ACTN3 RR)",
            None => "Unknown",
        }
        .into(),
        detail: format!("ACTN3 {}: alpha-actinin-3 in fast-twitch fibers.", gt_text(g, 1815739)),
        confidence: if actn3.is_some() { Confidence::High } else { Confidence::Low },
    });

    Traits { sex, eye_rgb, hair_rgb, skin_rgb, skin_index, texture, thickness, freckles, balding, list }
}

/// One-line summary for the likeness caption.
pub fn summary(t: &Traits) -> String {
    let v = |k: Kind| t.list.iter().find(|x| x.kind == k).map(|x| x.value.clone()).unwrap_or_default();
    format!("{} eyes · {} hair, {} · {} skin · freckles: {}", v(Kind::Eyes), v(Kind::HairColor).split(" —").next().unwrap_or(""), v(Kind::HairTexture).to_lowercase(), v(Kind::Skin).to_lowercase(), v(Kind::Freckles).to_lowercase())
}
