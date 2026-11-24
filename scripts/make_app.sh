#!/usr/bin/env bash
# Builds dist/genesloppa.app (Apple Silicon) with the reference pack inside, plus a .dmg.
#
# From the project root:
#   scripts/make_app.sh --pack reference.g25pack [--font path/to/font.ttf]
# or build the pack in the same step:
#   scripts/make_app.sh --aadr path/to/v66.p1_HO.aadr.patch.PUB --g25 path/to/g25_individuals.txt
#
#   (with --aadr the estimated heatmap layer is built too, and reused by later builds)
#   --chip my_raw_dna.csv.gz   keep only SNPs that consumer chips test (pack ~10x smaller);
#                              any raw DNA file works, it is only used for its SNP list
#
# Heatmap data is built into the app. Optional overrides:
#   --map world.g25map | --shp ne_50m_admin_0_countries.shp
#   --places places.tsv | --anno v66.p1_HO.aadr.PUB.anno --ind modern_ind.csv,ancient_ind.csv
#
# Options: --out DIR (default: dist)   --per-pop N (default 25, only with --aadr)
set -euo pipefail

PACK="" AADR="" G25="" FONT="" OUT="dist" PER_POP=25 CHIP="" MAP="" SHP="" PLACES="" ANNO="" IND=""
while [[ $# -gt 0 ]]; do
  case "$1" in
    --pack) PACK="$2"; shift 2 ;;
    --aadr) AADR="$2"; shift 2 ;;
    --g25) G25="$2"; shift 2 ;;
    --font) FONT="$2"; shift 2 ;;
    --out) OUT="$2"; shift 2 ;;
    --per-pop) PER_POP="$2"; shift 2 ;;
    --chip) CHIP="$2"; shift 2 ;;
    --map) MAP="$2"; shift 2 ;;
    --shp) SHP="$2"; shift 2 ;;
    --places) PLACES="$2"; shift 2 ;;
    --anno) ANNO="$2"; shift 2 ;;
    --ind) IND="$2"; shift 2 ;;
    *) echo "unknown option $1"; exit 1 ;;
  esac
done

[[ "$(uname -s)" == "Darwin" ]] || { echo "Run this on macOS."; exit 1; }
command -v cargo >/dev/null || { echo "Rust not found. Install it: curl https://sh.rustup.rs -sSf | sh"; exit 1; }

TARGET=aarch64-apple-darwin
rustup target add "$TARGET" >/dev/null 2>&1 || true
echo "==> Building for $TARGET"
cargo build --release --target "$TARGET"
BIN="target/$TARGET/release/g25app"

if [[ -z "$PACK" ]]; then
  [[ -n "$AADR" && -n "$G25" ]] || { echo "Give --pack FILE, or --aadr PREFIX and --g25 SHEET to build one."; exit 1; }
  PACK="$OUT/reference.g25pack"
  mkdir -p "$OUT"
  echo "==> Building reference pack"
  "$BIN" pack --aadr "$AADR" --g25 "$G25" --out "$PACK" --per-pop "$PER_POP" ${CHIP:+--chip "$CHIP"}
fi
[[ -f "$PACK" ]] || { echo "Reference pack not found: $PACK"; exit 1; }

mkdir -p "$OUT"
# Estimated heatmap layer: AADR populations without official G25 coordinates (needs the AADR
# files once; the result is kept in $OUT/extras and reused by later builds)
if [[ -n "$AADR" ]]; then
  echo "==> Building the estimated heatmap layer (a few minutes)"
  "$BIN" extras --aadr "$AADR" --pack "$PACK" --locations data/aadr_locations.tsv --out "$OUT/extras"
fi
if [[ -z "$MAP" && -n "$SHP" ]]; then
  MAP="$OUT/world.g25map"; echo "==> Building map from $SHP"; "$BIN" mapdata --shp "$SHP" --out "$MAP"
fi
if [[ -z "$PLACES" && -n "$ANNO" && -n "$IND" ]]; then
  PLACES="$OUT/places.tsv"; echo "==> Building population locations"; "$BIN" places --anno "$ANNO" --ind "$IND" --out "$PLACES"
fi

APP="$OUT/genesloppa.app"
echo "==> Assembling $APP"
rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources/fonts"
cp "$BIN" "$APP/Contents/MacOS/g25app"
cp "$PACK" "$APP/Contents/Resources/reference.g25pack"
# map + population locations are built into the binary; these only override them
if [[ -n "$MAP" ]]; then cp "$MAP" "$APP/Contents/Resources/world.g25map"; fi
if [[ -n "$PLACES" ]]; then cp "$PLACES" "$APP/Contents/Resources/places.tsv"; fi
if [[ -d "$OUT/extras" ]]; then cp "$OUT"/extras/extra_* "$APP/Contents/Resources/"; fi
# app icon, drawn by the app itself (src/logo.rs)
"$BIN" icon --out "$APP/Contents/Resources/genesloppa.icns" >/dev/null

FONT_KEY=""
# no --font: use the first font file in the project's fonts/ folder (the app font is Johnston)
if [[ -z "$FONT" ]]; then
  FONT="$(ls fonts/*.{ttf,otf,ttc} 2>/dev/null | head -1 || true)"
fi
if [[ -n "$FONT" ]]; then
  [[ -f "$FONT" ]] || { echo "Font file not found: $FONT"; exit 1; }
  cp "$FONT" "$APP/Contents/Resources/fonts/"
  # PostScript name, so the app can ask AppKit for exactly this font
  PS_NAME="$(mdls -raw -name com_apple_ats_name_postscript "$FONT" 2>/dev/null | tr -d '()",' | xargs | awk '{print $1}')"
  if [[ -n "$PS_NAME" && "$PS_NAME" != "null" ]]; then
    echo "    font PostScript name: $PS_NAME"
    FONT_KEY="<key>G25FontName</key><string>$PS_NAME</string>"
  else
    echo "    warning: couldn't read the font's PostScript name; the app will try common Johnston names"
  fi
else
  echo "    note: no --font given; the app will fall back to the system font"
fi

cat > "$APP/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleName</key><string>genesloppa</string>
  <key>CFBundleDisplayName</key><string>genesloppa</string>
  <key>CFBundleIdentifier</key><string>io.github.genesloppa</string>
  <key>CFBundleExecutable</key><string>g25app</string>
  <key>CFBundleIconFile</key><string>genesloppa</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleShortVersionString</key><string>0.1.0</string>
  <key>CFBundleVersion</key><string>1</string>
  <key>LSMinimumSystemVersion</key><string>11.0</string>
  <key>LSArchitecturePriority</key><array><string>arm64</string></array>
  <key>NSHighResolutionCapable</key><true/>
  <key>ATSApplicationFontsPath</key><string>fonts</string>
  $FONT_KEY
</dict>
</plist>
PLIST

# Apple Silicon refuses to run unsigned code; an ad-hoc signature is enough for local use
codesign --force --deep --sign - "$APP"

echo "==> Built $APP ($(du -sh "$APP" | cut -f1))"

# Drag-to-install disk image: the app + a shortcut to /Applications
DMG="$OUT/genesloppa.dmg"
STAGE="$(mktemp -d)"
cp -R "$APP" "$STAGE/"
ln -s /Applications "$STAGE/Applications"
rm -f "$DMG"
hdiutil create -volname "genesloppa" -srcfolder "$STAGE" -fs HFS+ -format UDZO -ov "$DMG" >/dev/null
rm -rf "$STAGE"
echo "==> Done: $DMG ($(du -sh "$DMG" | cut -f1))"
echo "    Share/open this file — users just drag the app into Applications."
