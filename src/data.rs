//! Built-in G25 source sets, compiled into the binary.
//! MODERN/ANCIENT feed the heatmap and the Distance tab and are the modeling page's default
//! built-in sources; the IllustrativeDNA era sheets replace them when turned on.

pub const ANCIENT: &str = include_str!("../data/g25_ancient_averages_scaled.txt");
pub const MODERN: &str = include_str!("../data/g25_modern_averages_scaled.txt");

/// The default built-in source sets on the modeling page (G25 population averages).
pub const G25_SETS: [&str; 2] = ["Modern", "Ancient"];
pub fn g25_set(index: usize) -> &'static str {
    if index == 0 { MODERN } else { ANCIENT }
}

/// IllustrativeDNA era sets (off by default; they replace Modern/Ancient when shown).
/// Shown as a tooltip on the era controls: how the sets were made, and their limits.
pub const ERA_NOTE: &str = "Approximate era sets, sorted by hand from Illustrative DNA samples and groups. \
Each sample goes in the era that covers most of its date range (Bronze Age 3300-1200 BC, Iron Age 1200 BC-AD 1, \
Late Antiquity 100 BC-AD 700, Middle Ages AD 500-1500), so the eras overlap, samples near a boundary could \
fit either, and post-1500 samples are left out. Use them for a first look; for anything you want to rely on, \
use the Ancient G25 averages.";
pub const BUILTIN: [&str; 6] = ["Mesolithic", "Neolithic", "Bronze Age", "Iron Age", "Late Antiquity", "Middle Ages"];
const ERAS: [&str; 6] = [
    include_str!("../data/era_mesolithic.txt"),
    include_str!("../data/era_neolithic.txt"),
    include_str!("../data/era_bronze_age.txt"),
    include_str!("../data/era_iron_age.txt"),
    include_str!("../data/era_late_antiquity.txt"),
    include_str!("../data/era_middle_ages.txt"),
];

/// Source text for a built-in set (one G25 line per population).
pub fn builtin(index: usize) -> String {
    ERAS[index.min(ERAS.len() - 1)].to_string()
}

pub fn count_lines(text: &str) -> usize {
    text.lines().filter(|l| !l.trim().is_empty()).count()
}
