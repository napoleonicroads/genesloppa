//! Consumer raw DNA files: 23andMe, AncestryDNA, MyHeritage, FTDNA, LivingDNA.
//! Accepts plain text, .gz and .zip (decompressed with the gzip/unzip that ship with macOS).

use std::collections::HashMap;
use std::hash::{BuildHasherDefault, Hasher};
use std::io::{BufRead, BufReader, Read};
use std::process::{Command, Stdio};

/// rsID number (rs123 -> 123) -> genotype letters
pub type RawGenotypes = HashMap<u32, [u8; 2], BuildHasherDefault<RsidHasher>>;

/// Keys are plain integers, so a multiply-xor hash is plenty; std's SipHash is several times
/// slower here.
#[derive(Default)]
pub struct RsidHasher(u64);
impl Hasher for RsidHasher {
    #[inline]
    fn write(&mut self, bytes: &[u8]) {
        for &b in bytes {
            self.write_u64(b as u64);
        }
    }
    #[inline]
    fn write_u32(&mut self, n: u32) {
        self.write_u64(n as u64);
    }
    #[inline]
    fn write_u64(&mut self, n: u64) {
        let x = (self.0 ^ n).wrapping_mul(0x9E37_79B9_7F4A_7C15);
        self.0 = x ^ (x >> 29);
    }
    #[inline]
    fn finish(&self) -> u64 {
        self.0
    }
}

/// X-chromosome markers are stored under `rsid | X_FLAG`, so they never mix with the autosomal
/// SNPs used for ancestry (the reference pack is autosomal) but stay available for sex inference
/// and X-linked health markers.
pub const X_FLAG: u32 = 1 << 31;

pub fn is_acgt(b: u8) -> bool {
    matches!(b, b'A' | b'C' | b'G' | b'T')
}

pub fn read_raw_file(path: &str) -> Result<RawGenotypes, String> {
    let mut f = std::fs::File::open(path).map_err(|e| format!("{path}: {e}"))?;
    let mut magic = [0u8; 4];
    let n = f.read(&mut magic).map_err(|e| e.to_string())?;
    let reader: Box<dyn BufRead> = if n >= 2 && magic[..2] == [0x1f, 0x8b] {
        Box::new(BufReader::new(spawn_stdout(Command::new("/usr/bin/gzip").args(["-dc", path]))?))
    } else if n == 4 && magic == *b"PK\x03\x04" {
        // first file inside the zip (that's where every vendor puts the data)
        Box::new(BufReader::new(spawn_stdout(Command::new("/usr/bin/unzip").args(["-p", path]))?))
    } else {
        Box::new(BufReader::with_capacity(1 << 20, std::fs::File::open(path).map_err(|e| e.to_string())?))
    };
    let g = parse(reader);
    if g.len() < 10_000 {
        return Err(format!(
            "only {} usable SNPs found — is this a raw DNA export (23andMe, AncestryDNA, MyHeritage, FTDNA)?",
            g.len()
        ));
    }
    Ok(g)
}

fn spawn_stdout(cmd: &mut Command) -> Result<impl Read, String> {
    let child = cmd.stdout(Stdio::piped()).stderr(Stdio::null()).spawn().map_err(|e| format!("cannot decompress: {e}"))?;
    Ok(child.stdout.unwrap())
}

/// The ASCII characters `char::is_whitespace` accepts (note: includes vertical tab, unlike
/// `u8::is_ascii_whitespace`).
#[inline]
fn is_ws(b: u8) -> bool {
    matches!(b, b' ' | b'\t' | b'\n' | 0x0B | 0x0C | b'\r')
}
/// `s.trim().trim_matches('"')` for ASCII `s`.
#[inline]
fn trim_field(s: &str) -> &str {
    let b = s.as_bytes();
    let (mut i, mut j) = (0, b.len());
    while i < j && is_ws(b[i]) {
        i += 1;
    }
    while j > i && is_ws(b[j - 1]) {
        j -= 1;
    }
    while i < j && b[i] == b'"' {
        i += 1;
    }
    while j > i && b[j - 1] == b'"' {
        j -= 1;
    }
    &s[i..j]
}

pub fn parse(mut reader: impl BufRead) -> RawGenotypes {
    let mut map = RawGenotypes::with_capacity_and_hasher(700_000, Default::default());
    // one reused line buffer and a fixed field array instead of a String + Vec per line
    let mut buf = String::with_capacity(128);
    loop {
        buf.clear();
        match reader.read_line(&mut buf) {
            Ok(0) | Err(_) => break, // EOF, or a non-UTF-8 line (stops, as `lines()` did)
            Ok(_) => {}
        }
        let line = buf.strip_suffix('\n').map(|l| l.strip_suffix('\r').unwrap_or(l)).unwrap_or(&buf);
        let mut parts = [""; 5];
        let mut count = 0usize;
        if line.is_ascii() {
            // fast path for ASCII lines (every real export): split and trim on bytes
            let b = line.as_bytes();
            if b.first() == Some(&b'#') || b.iter().all(|&c| is_ws(c)) {
                continue;
            }
            let mut start = 0;
            for i in 0..=b.len() {
                if i == b.len() || b[i] == b',' || b[i] == b'\t' {
                    if count < 5 {
                        parts[count] = trim_field(&line[start..i]);
                    }
                    count += 1;
                    start = i + 1;
                }
            }
        } else {
            if line.starts_with('#') || line.trim().is_empty() {
                continue;
            }
            for f in line.split([',', '\t']) {
                if count < 5 {
                    parts[count] = f.trim().trim_matches('"');
                }
                count += 1;
            }
        }
        let (rsid, chrom, a, b) = match count {
            5 => (parts[0], parts[1], parts[3], parts[4]), // AncestryDNA: allele1, allele2 columns
            n if n >= 4 => {
                let g = parts[3];
                match g.len() {
                    2 => (parts[0], parts[1], &g[..1], &g[1..]),
                    1 => (parts[0], parts[1], g, g), // hemizygous X call (e.g. 23andMe males)
                    _ => continue,
                }
            }
            _ => continue,
        };
        let Some(id) = rsid.strip_prefix("rs").and_then(|x| x.parse::<u32>().ok()) else { continue };
        // autosomes for ancestry; X kept for X-linked health markers (G6PD). The reference pack is
        // autosomal, so X entries never enter the G25 conversion.
        let is_x = chrom == "X" || chrom == "23";
        if !(matches!(chrom.parse::<u8>(), Ok(1..=22)) || is_x) {
            continue;
        }
        let (a, b) = (a.as_bytes(), b.as_bytes());
        if a.len() != 1 || b.len() != 1 || !is_acgt(a[0]) || !is_acgt(b[0]) {
            continue;
        }
        map.insert(if is_x { id | X_FLAG } else { id }, [a[0], b[0]]);
    }
    map
}
