# genesloppa

An all in one tool for amateur "genealogy", coded fully in Rust. Quicker than its peers and comes with preinstalled sources.

A Mac app that turns a consumer raw DNA file into Global25 coordinates, then lets you do
things with them: a distance heatmap, Vahaduo-style admixture and distance modeling, qpAdm on
real genotypes, and a few trait and health readouts.

It's written in Rust with no crates. The UI talks to AppKit and Core Graphics directly through
the Objective-C runtime, so there's no web view and nothing to install besides the app.
Apple Silicon only.

Everything runs on your Mac. The app makes no network connections, so your DNA file never
leaves your computer.

## Download

Get **genesloppa.dmg** from the [latest release](https://github.com/napoleonicroads/genesloppa/releases/latest),
open it and drag genesloppa into Applications. It needs a Mac with Apple Silicon (M1 or newer)
and macOS 11 or later.

The app isn't signed with an Apple developer account, so macOS blocks it the first time:

1. Open genesloppa. When macOS says it can't be opened, click **Done**.
2. Go to **System Settings > Privacy & Security**, scroll down, click **Open Anyway** next to
   the genesloppa message and enter your password.
3. Open genesloppa again and click **Open**.

If macOS says the app "is damaged", run this once in Terminal and open it again:

```
xattr -dr com.apple.quarantine /Applications/genesloppa.app
```

## Pages

- **Converter**: 23andMe, AncestryDNA, MyHeritage, FTDNA or LivingDNA files (.txt, .csv, .gz
  or .zip) to G25 coordinates, with a quality estimate and your closest populations.
- **Heatmap**: your distance to every placed population, modern or ancient. Drag to pan,
  scroll or pinch to zoom. Hollow "(est.)" pins are AADR populations with no official G25
  coordinates, projected by the app.
- **G25 modeling**: a port of Vahaduo's admixture ("Single" mode, R#P) and Distance tab. Use the
  built-in Modern and Ancient averages, the era sets, or paste your own sources.
- **qpAdm**: the same model as ADMIXTOOLS 2, run on the genotypes in the reference pack.
  Suggest sources from G25, rotate through 1 to 3 source models, check the Right set before a
  run, add your own populations (raw files or merged groups), and export an R script that runs
  the same model in ADMIXTOOLS 2.
- **Physical characteristics**: eyes, hair, skin, freckles, hairline and so on, plus a
  cartoon likeness.
- **Health info**: a short list of well-studied SNPs. These are common variants that nudge
  the odds, not a diagnosis.

## Building (ONLY FOR DEVELOPERS!)

This is only for working on the code. The download above is ready to use and already contains
everything.

You need macOS on Apple Silicon and Rust. The app also needs a reference pack (the genotypes
behind the converter and qpAdm). It's too big for this repo, but the release download has one
inside, so the quickest way is to install the app and build against its pack:

```
mkdir -p dist/extras && cp /Applications/genesloppa.app/Contents/Resources/extra_* dist/extras/
scripts/make_app.sh --pack /Applications/genesloppa.app/Contents/Resources/reference.g25pack
open dist/genesloppa.app
```

(The first line copies the estimated heatmap layer, the hollow "(est.)" pins, from the app.)

To build the pack yourself instead, you need the AADR "HO" dataset and a Global25 individuals
sheet:

```
scripts/make_app.sh --aadr path/to/v66.p1_HO.aadr.patch.PUB \
    --g25 path/to/g25_individuals.txt --chip path/to/any_raw_dna_file.csv.gz
open dist/genesloppa.app
```

- `--g25` is an individuals sheet (`Pop:SampleID,...`). The ancient and modern "scaled"
  sheets can just be concatenated.
- `--chip` keeps only the SNPs consumer chips test, which makes the pack about 10x smaller.
  Any raw DNA file works; only its SNP list is used.
- With `--aadr`, the build also makes the estimated heatmap layer and caches it in
  `dist/extras/`.
- To reuse a pack you already built: `scripts/make_app.sh --pack dist/reference.g25pack`.

While working on the code, `cargo run --release` starts the app with the data in
`dist/genesloppa.app/Contents/Resources` (set in `.cargo/config.toml`). `cargo test --release` runs
the tests; the qpAdm ones need `dist/reference.g25pack` and skip without it.

## Command line

The same engines, without the window:

```
g25app convert --raw FILE [--name N] [--pack reference.g25pack]
g25app model --target me.txt --sources sources.txt [--npop 3-5] [--cycles 1] [--adc 0.25 --rc]
g25app qpadm --pack reference.g25pack --target POP --left A,B --right R0,R1,... [--raw FILE]
g25app health --raw FILE
g25app traits --raw FILE
g25app avatar --raw FILE --out me.svg
g25app icon --out genesloppa.icns                          (the app icon; .svg works too)
g25app pack | extras | mapdata | places | locations ...    (building the data)
```

## Data

- `data/world.g25map`: Natural Earth 1:50m country outlines (public domain).
- `data/places.tsv`, `data/aadr_locations.tsv`: population and sample locations from the AADR
  and Poseidon annotations. A few known mistakes in them are fixed in `geo::PLACE_FIXES`.
- `data/g25_*_averages_scaled.txt`: Global25 population averages.
- `data/era_*.txt`: era averages used by the modeling page's era sets.

These files come from other people's work and keep their own terms; the MIT license below
doesn't cover them. The same goes for the reference pack inside the release download, which is
built from AADR genotypes and Global25 data. Check their terms before you redistribute them.

## Font

The UI uses Johnston if a font file is in `fonts/`, otherwise the system font. Johnston is a
licensed typeface and isn't included.

## Code

| file | what |
|---|---|
| `src/gui.rs` | the app: sidebar, pages, map, result views |
| `src/gui/shots.rs` | renders pages to PNG (`--features shots`) |
| `src/engine.rs` | raw genotypes to G25: overlap and strand, PCA, projection, calibration |
| `src/linalg.rs` | packed genotypes, the matrix kernels, small linear algebra |
| `src/pack.rs`, `src/raw.rs` | reference pack format and builder, raw file parsing |
| `src/model.rs` | Vahaduo admixture and distance |
| `src/qpadm.rs` | f2 blocks, qpAdm fit, jackknife, your own populations |
| `src/geo.rs`, `src/extras.rs` | map data, locations, heat raster, estimated layer |
| `src/traits.rs`, `src/avatar.rs` | physical traits and the likeness drawing |
| `src/logo.rs`, `src/gui/icon.rs` | the logo, and the app icon drawn from it |
| `src/health.rs` | Health info SNPs |
| `src/data.rs` + `data/` | built-in source sets |

## Contributing

Bug reports and pull requests are welcome; see `CONTRIBUTING.md`.

## Credits

- [Vahaduo](https://vahaduo.github.io/vahaduo/), whose admixture and distance tools the modeling
  page is ported from.
- [ADMIXTOOLS 2](https://uqrmaie1.github.io/admixtools/) (Maier et al. 2023), which the qpAdm
  page follows.
- The [Allen Ancient DNA Resource](https://reich.hms.harvard.edu/allen-ancient-dna-resource-aadr-downloadable-genotypes-present-day-and-ancient-dna-data)
  (Mallick et al. 2024) and the [Poseidon](https://www.poseidon-adna.org/) annotations, for
  genotypes and sample locations.
- Global25 by Davidski ([Eurogenes](https://eurogenes.blogspot.com/)), for the coordinates
  and population averages.
- Illustrative DNA, for the era averages.
- [Natural Earth](https://www.naturalearthdata.com/), for the map.

## License

The code is MIT licensed (see `LICENSE`). The files in `data/` and the reference pack are not
covered by it; see Data above.
