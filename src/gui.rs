//! The macOS app. AppKit and Core Graphics are called straight through the Objective-C runtime
//! (the `msg!` macro) and the C APIs.
//!
//! Sidebar pages: Converter, Heatmap, G25 modeling, qpAdm, Physical characteristics, Health info.
//! UI text uses Johnston if a font file is in `fonts/`, otherwise the system font; numbers use the
//! system monospaced font.

#![allow(non_upper_case_globals, unused_parens, clippy::missing_safety_doc)]

use crate::engine::{self, Report};
use crate::geo::{self, Heat, World};
use crate::pack::Pack;
use crate::raw::RawGenotypes;
use crate::avatar;
use crate::traits::{self, Kind, Sex, Texture, Traits};
use crate::{data, health, model, qpadm, raw};
use std::collections::HashMap;
use std::ffi::{c_char, c_void, CStr, CString};
use std::sync::{Arc, Mutex, OnceLock};

type Id = *mut c_void;
type Sel = *const c_void;
const NIL: Id = std::ptr::null_mut();

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct NSRect {
    x: f64,
    y: f64,
    w: f64,
    h: f64,
}
#[repr(C)]
#[derive(Clone, Copy, Default)]
struct NSPoint {
    x: f64,
    y: f64,
}
#[repr(C)]
#[derive(Clone, Copy)]
struct NSSize {
    w: f64,
    h: f64,
}
fn rect(x: f64, y: f64, w: f64, h: f64) -> NSRect {
    NSRect { x, y, w, h }
}

#[cfg_attr(target_os = "macos", link(name = "objc"))]
extern "C" {
    fn objc_getClass(name: *const c_char) -> Id;
    fn sel_registerName(name: *const c_char) -> Sel;
    fn objc_msgSend();
    fn objc_allocateClassPair(superclass: Id, name: *const c_char, extra: usize) -> Id;
    fn objc_registerClassPair(cls: Id);
    fn class_addMethod(cls: Id, name: Sel, imp: *const c_void, types: *const c_char) -> bool;
    fn objc_autoreleasePoolPush() -> *mut c_void;
    fn objc_autoreleasePoolPop(pool: *mut c_void);
}
#[cfg_attr(target_os = "macos", link(name = "AppKit", kind = "framework"))]
extern "C" {}
#[cfg_attr(target_os = "macos", link(name = "Foundation", kind = "framework"))]
extern "C" {}

// Core Graphics (C API)
type CG = *mut c_void;
#[cfg_attr(target_os = "macos", link(name = "CoreText", kind = "framework"))]
extern "C" {
    fn CTFontManagerRegisterFontsForURL(url: Id, scope: u32, err: *mut Id) -> bool;
}
#[cfg_attr(target_os = "macos", link(name = "CoreGraphics", kind = "framework"))]
extern "C" {
    fn CGContextSaveGState(c: CG);
    fn CGContextRestoreGState(c: CG);
    fn CGContextTranslateCTM(c: CG, x: f64, y: f64);
    fn CGContextScaleCTM(c: CG, x: f64, y: f64);
    fn CGContextSetRGBFillColor(c: CG, r: f64, g: f64, b: f64, a: f64);
    fn CGContextSetRGBStrokeColor(c: CG, r: f64, g: f64, b: f64, a: f64);
    fn CGContextFillRect(c: CG, r: NSRect);
    fn CGContextStrokeRect(c: CG, r: NSRect);
    fn CGContextBeginPath(c: CG);
    fn CGContextMoveToPoint(c: CG, x: f64, y: f64);
    fn CGContextAddLineToPoint(c: CG, x: f64, y: f64);
    fn CGContextClosePath(c: CG);
    fn CGContextFillPath(c: CG);
    fn CGContextStrokePath(c: CG);
    fn CGContextClip(c: CG);
    fn CGContextSetLineWidth(c: CG, w: f64);
    fn CGContextAddRect(c: CG, r: NSRect);
    fn CGContextAddEllipseInRect(c: CG, r: NSRect);
    fn CGContextDrawPath(c: CG, mode: i32);
    fn CGContextDrawImage(c: CG, r: NSRect, img: CG);
    fn CGContextSetInterpolationQuality(c: CG, q: i32);
    fn CGContextAddCurveToPoint(c: CG, c1x: f64, c1y: f64, c2x: f64, c2y: f64, x: f64, y: f64);
    fn CGContextAddQuadCurveToPoint(c: CG, cx: f64, cy: f64, x: f64, y: f64);
    fn CGContextFillEllipseInRect(c: CG, r: NSRect);
    fn CGContextStrokeEllipseInRect(c: CG, r: NSRect);
    fn CGContextSetLineCap(c: CG, cap: i32);
    fn CGContextSetLineJoin(c: CG, join: i32);
    fn CGPathCreateMutable() -> CG;
    fn CGPathMoveToPoint(p: CG, m: *const c_void, x: f64, y: f64);
    fn CGPathAddLineToPoint(p: CG, m: *const c_void, x: f64, y: f64);
    fn CGPathCloseSubpath(p: CG);
    fn CGContextAddPath(c: CG, p: CG);
    fn CGColorSpaceCreateDeviceRGB() -> CG;
    fn CGColorSpaceRelease(cs: CG);
    fn CGBitmapContextCreate(data: *mut c_void, w: usize, h: usize, bpc: usize, bpr: usize, cs: CG, info: u32) -> CG;
    fn CGBitmapContextCreateImage(c: CG) -> CG;
    fn CGContextRelease(c: CG);
    fn CGImageRelease(img: CG);
}

/// Selector names are NUL-terminated on the stack (every message send goes through here);
/// long ones fall back to a CString.
fn with_cstr<R>(name: &str, f: impl FnOnce(*const c_char) -> R) -> R {
    let mut buf = [0u8; 128];
    if name.len() < buf.len() && !name.as_bytes().contains(&0) {
        buf[..name.len()].copy_from_slice(name.as_bytes());
        f(buf.as_ptr() as *const c_char)
    } else {
        let c = CString::new(name).unwrap();
        f(c.as_ptr())
    }
}
fn sel(name: &str) -> Sel {
    with_cstr(name, |p| unsafe { sel_registerName(p) })
}
fn cls(name: &str) -> Id {
    with_cstr(name, |p| unsafe { objc_getClass(p) })
}

/// msg!(RetType; receiver, "selector:", arg => ArgType, ...)
macro_rules! msg {
    ($ret:ty; $obj:expr, $sel:expr $(, $arg:expr => $t:ty)*) => {{
        let f: unsafe extern "C" fn(Id, Sel $(, $t)*) -> $ret =
            std::mem::transmute(objc_msgSend as unsafe extern "C" fn());
        f($obj, sel($sel) $(, $arg)*)
    }};
}

fn ns(s: &str) -> Id {
    let c = if s.contains('\0') { CString::new(s.replace('\0', "")) } else { CString::new(s) }.unwrap();
    unsafe { msg![Id; cls("NSString"), "stringWithUTF8String:", c.as_ptr() => *const c_char] }
}
fn from_ns(s: Id) -> String {
    if s.is_null() {
        return String::new();
    }
    unsafe {
        let p = msg![*const c_char; s, "UTF8String"];
        if p.is_null() { String::new() } else { CStr::from_ptr(p).to_string_lossy().into_owned() }
    }
}
unsafe fn ns_array(items: &[Id]) -> Id {
    msg![Id; cls("NSArray"), "arrayWithObjects:count:", items.as_ptr() => *const Id, items.len() => usize]
}
unsafe fn color(r: f64, g: f64, b: f64, a: f64) -> Id {
    msg![Id; cls("NSColor"), "colorWithSRGBRed:green:blue:alpha:", r => f64, g => f64, b => f64, a => f64]
}
unsafe fn sys_color(name: &str) -> Id {
    msg![Id; cls("NSColor"), name]
}
unsafe fn set_layer_bg(view: Id, c: Id, radius: f64) {
    msg![(); view, "setWantsLayer:", true => bool];
    let layer = msg![Id; view, "layer"];
    let cg = if c.is_null() { NIL } else { msg![Id; c, "CGColor"] };
    msg![(); layer, "setBackgroundColor:", cg => Id];
    msg![(); layer, "setCornerRadius:", radius => f64];
}

// ================================================================ state
const SIDEBAR_W: f64 = 236.0;
const SIDEBAR_COLLAPSED: f64 = 60.0;
const WIN_W: f64 = 1080.0;
const WIN_H: f64 = 700.0;
const NAV: [(&str, &str); 6] = [
    ("Converter", "arrow.left.arrow.right"),
    ("Heatmap", "map"),
    ("G25 modeling", "chart.pie"),
    ("qpAdm", "function"),
    ("Physical characteristics", "person"),
    ("Health info", "heart.text.square"),
];
const PAGES: usize = NAV.len();

#[derive(Default)]
struct Shared {
    // converter
    file: Option<String>,
    status: String,
    conv_text: Option<String>,
    line: Option<String>,
    name: String,
    busy: bool,
    // shared results
    coords: Option<Vec<f64>>,
    health_text: Option<String>,
    /// qpAdm page: latest report, busy flag and progress line
    qp: Option<QpReport>,
    qp_busy: bool,
    qp_progress: String,
    /// Health page cards, ranked (most alarming first, then medicine notes, then fun facts)
    health_items: Option<Vec<health::Item>>,
    traits: Option<Traits>,
    // modeling
    sources: String,
    sources_label: String,
    builtin: isize,
    model_busy: bool,
    model_result: Option<ModelView>,
    model_error: Option<String>,
    /// G25 modeling page: false = Admixture, true = Distance
    dist_mode: bool,
    /// heatmap: population dots + labels hidden
    hide_dots: bool,
    /// built-in source sets shown (on by default)
    show_builtin: bool,
    /// IllustrativeDNA era sets shown instead of the Modern/Ancient G25 averages (off by default)
    show_illus: bool,
    /// Distance gradient: false = auto steepness (stretched over the rows shown, the default),
    /// true = the user's own steepness from the slider
    dist_manual: bool,
    /// manual: distance gap from the closest row to full red
    dist_span: f64,
    dist_result: Option<DistView>,
    history: Vec<String>,
    target_filled: Option<String>,
    // heatmap
    heat: [Option<Heat>; 2],
    heat_note: String,
    map_mode: usize,
    hover: Option<usize>,
    view: MapView,
    // ui
    page: usize,
    collapsed: bool,
}

#[derive(Default, Clone, Copy)]
struct MapView {
    k: f64,
    k0: f64,
    tx: f64,
    ty: f64,
    init: bool,
    last: NSPoint,
}

#[derive(Clone)]
struct ModelView {
    target: String,
    distance: f64,
    npop: usize,
    rows: Vec<(String, f64)>,
    footer: String,
    text: String,
}

#[derive(Clone)]
struct DistView {
    target: String,
    pool: String,
    total: usize,
    rows: Vec<(String, f64)>,
    text: String,
}

static STATE: Mutex<Option<Shared>> = Mutex::new(None);
static PACK: OnceLock<Result<Pack, String>> = OnceLock::new();
static WORLD: OnceLock<Result<World, String>> = OnceLock::new();
static PLACES: OnceLock<Result<HashMap<String, (f64, f64)>, String>> = OnceLock::new();
static RAWG: Mutex<Option<Arc<RawGenotypes>>> = Mutex::new(None);
/// CGImage pointers for the two heat rasters (created lazily on the main thread)
static HEAT_IMG: Mutex<[usize; 2]> = Mutex::new([0, 0]);
/// The world outline as one CGPath, built once (the map never changes) and reused by every
/// frame's fill, clip and stroke instead of re-sending every vertex three times per frame.
static LAND_PATH: OnceLock<usize> = OnceLock::new();
/// Label widths for the map (same font every time), measured once per name.
static LABEL_W: Mutex<Option<HashMap<String, f64>>> = Mutex::new(None);

fn with_state<R>(f: impl FnOnce(&mut Shared) -> R) -> R {
    let mut g = STATE.lock().unwrap();
    f(g.get_or_insert_with(|| Shared {
        status: "Choose your raw DNA file to begin".into(),
        name: "me".into(),
        sources: data::MODERN.to_string(),
        sources_label: builtin_label(G25_BASE),
        builtin: G25_BASE,
        show_builtin: true,
        dist_span: 0.10,
        heat_note: "Convert your DNA file to see how close you are to each population.".into(),
        ..Default::default()
    }))
}

// ================================================================ widgets
#[allow(dead_code)]
struct Widgets {
    win: usize,
    sidebar: usize,
    content: usize,
    pages: [usize; PAGES],
    nav: [usize; PAGES],
    side_title: usize,
    side_footer: usize,
    // converter
    name: usize,
    file: usize,
    status: usize,
    convert: usize,
    conv_text: usize,
    copy: usize,
    // heatmap
    map: usize,
    mode: usize,
    closest: usize,
    // modeling
    target: usize,
    src_label: usize,
    /// IllustrativeDNA era picker
    builtin: usize,
    /// Modern / Ancient picker
    builtin_g25: usize,
    illus_toggle: usize,
    npop: usize,
    cycles: usize,
    agg: usize,
    adc: usize,
    run: usize,
    bars: usize,
    model_mode: usize,
    builtin_toggle: usize,
    /// admixture-only controls (hidden in Distance mode)
    admix: [usize; 6],
    /// distance-only controls: "Show" label, count field, era picker
    dist_ctl: [usize; 3],
    /// Distance gradient: "Auto steepness" checkbox, steepness slider, value field (gap to full red)
    grad_ctl: [usize; 3],
    dist_scroll: usize,
    dist_view: usize,
    // physical characteristics + health
    traits_view: usize,
    health: usize,
}
static LIKENESS: OnceLock<(usize, usize)> = OnceLock::new();
static W: OnceLock<Widgets> = OnceLock::new();
static TOG: OnceLock<usize> = OnceLock::new();
static CONTROLLER: OnceLock<usize> = OnceLock::new();

fn request_refresh() {
    if let Some(&c) = CONTROLLER.get() {
        unsafe { msg![(); c as Id, "performSelectorOnMainThread:withObject:waitUntilDone:", sel("refresh:") => Sel, NIL => Id, false => bool] }
    }
}

fn resource(name: &str) -> String {
    if let Ok(d) = std::env::var("G25_DATA") {
        return format!("{d}/{name}");
    }
    let exe = std::env::current_exe().unwrap_or_default();
    let dir = exe.parent().map(|p| p.to_path_buf()).unwrap_or_default();
    for cand in [dir.join("../Resources").join(name), dir.join(name), std::path::PathBuf::from(name)] {
        if cand.exists() {
            return cand.to_string_lossy().into_owned();
        }
    }
    name.to_string()
}

// ---------------------------------------------------------------- fonts
fn ui_font(size: f64) -> Id {
    static NAME: OnceLock<Option<String>> = OnceLock::new();
    unsafe {
        let name = NAME.get_or_init(|| {
            let bundle = msg![Id; cls("NSBundle"), "mainBundle"];
            let configured = msg![Id; bundle, "objectForInfoDictionaryKey:", ns("G25FontName") => Id];
            let mut names = vec![];
            if !configured.is_null() {
                names.push(from_ns(configured));
            }
            // the app's font is Johnston (TfL's typeface; also sold as Johnston ITC / P22 Underground)
            names.extend([
                "Johnston100-Regular", "Johnston100", "Johnston", "JohnstonITCPro-Medium", "JohnstonITCPro-Regular", "Johnston ITC Pro",
                "JohnstonITCStd-Medium", "P22UndergroundPro-Medium", "P22UndergroundPro-Book", "P22 Underground",
            ].map(String::from));
            names.into_iter().find(|n| !msg![Id; cls("NSFont"), "fontWithName:size:", ns(n) => Id, 12.0f64 => f64].is_null())
        });
        if let Some(n) = name {
            return msg![Id; cls("NSFont"), "fontWithName:size:", ns(n) => Id, size => f64];
        }
        msg![Id; cls("NSFont"), "systemFontOfSize:", size => f64]
    }
}
unsafe fn mono(size: f64) -> Id {
    let f = msg![Id; cls("NSFont"), "monospacedSystemFontOfSize:weight:", size => f64, 0.0f64 => f64];
    if f.is_null() { msg![Id; cls("NSFont"), "userFixedPitchFontOfSize:", size => f64] } else { f }
}

/// Make font files in the app's `fonts` folder (and the project's `fonts/` under `cargo run`)
/// usable by this process, so the app font works without installing it.
unsafe fn register_fonts() {
    for dir in [resource("fonts"), "fonts".to_string()] {
        let Ok(rd) = std::fs::read_dir(&dir) else { continue };
        for e in rd.flatten() {
            let path = e.path();
            let ext = path.extension().and_then(|x| x.to_str()).unwrap_or("").to_ascii_lowercase();
            if ["ttf", "otf", "ttc"].contains(&ext.as_str()) {
                let p = path.canonicalize().unwrap_or(path);
                let url = msg![Id; cls("NSURL"), "fileURLWithPath:", ns(&p.to_string_lossy()) => Id];
                CTFontManagerRegisterFontsForURL(url, 1, std::ptr::null_mut()); // 1 = this process
            }
        }
    }
}

// ---------------------------------------------------------------- widget helpers
// All pages use top-left based layout: `top` is distance from the parent's top edge.
unsafe fn place(parent_h: f64, x: f64, top: f64, w: f64, h: f64) -> NSRect {
    rect(x, parent_h - top - h, w, h)
}
unsafe fn add(parent: Id, v: Id, mask: usize) -> Id {
    msg![(); v, "setAutoresizingMask:", mask => usize];
    msg![(); parent, "addSubview:", v => Id];
    v
}
const M_W: usize = 2; // width sizable
const M_H: usize = 16; // height sizable
const M_LEFT: usize = 1; // flexible left margin (stick right)
const M_BOTTOM: usize = 8; // flexible bottom margin (stick top), non-flipped
const M_TOP: usize = 32;

unsafe fn label(parent: Id, text: &str, r: NSRect, size: f64, mask: usize) -> Id {
    let l = msg![Id; cls("NSTextField"), "labelWithString:", ns(text) => Id];
    msg![(); l, "setFrame:", r => NSRect];
    msg![(); l, "setFont:", ui_font(size) => Id];
    add(parent, l, mask)
}
unsafe fn button(parent: Id, title: &str, r: NSRect, action: &str, mask: usize) -> Id {
    let ctl = *CONTROLLER.get().unwrap() as Id;
    let b = msg![Id; cls("NSButton"), "buttonWithTitle:target:action:", ns(title) => Id, ctl => Id, sel(action) => Sel];
    msg![(); b, "setFrame:", r => NSRect];
    msg![(); b, "setFont:", ui_font(14.0) => Id];
    add(parent, b, mask)
}
unsafe fn field(parent: Id, r: NSRect, value: &str, font: Id, mask: usize) -> Id {
    let f = msg![Id; msg![Id; cls("NSTextField"), "alloc"], "initWithFrame:", r => NSRect];
    msg![(); f, "setStringValue:", ns(value) => Id];
    msg![(); f, "setFont:", font => Id];
    add(parent, f, mask)
}
unsafe fn text_area(parent: Id, r: NSRect, font: Id, mask: usize) -> Id {
    let scroll = msg![Id; msg![Id; cls("NSScrollView"), "alloc"], "initWithFrame:", r => NSRect];
    msg![(); scroll, "setHasVerticalScroller:", true => bool];
    msg![(); scroll, "setBorderType:", 2usize => usize];
    let tv = msg![Id; msg![Id; cls("NSTextView"), "alloc"], "initWithFrame:", rect(0.0, 0.0, r.w - 4.0, r.h) => NSRect];
    msg![(); tv, "setEditable:", false => bool];
    msg![(); tv, "setSelectable:", true => bool];
    msg![(); tv, "setRichText:", false => bool];
    msg![(); tv, "setFont:", font => Id];
    msg![(); tv, "setTextContainerInset:", NSSize { w: 6.0, h: 8.0 } => NSSize];
    msg![(); tv, "setVerticallyResizable:", true => bool];
    msg![(); tv, "setHorizontallyResizable:", false => bool];
    msg![(); tv, "setAutoresizingMask:", M_W => usize];
    msg![(); tv, "setMinSize:", NSSize { w: 0.0, h: r.h } => NSSize];
    msg![(); tv, "setMaxSize:", NSSize { w: 1e7, h: 1e7 } => NSSize];
    msg![(); scroll, "setDocumentView:", tv => Id];
    add(parent, scroll, mask);
    tv
}
/// Long lines scroll sideways instead of wrapping (population names stay on one line).
unsafe fn no_wrap(tv: Id) {
    let sv = msg![Id; tv, "enclosingScrollView"];
    msg![(); sv, "setHasHorizontalScroller:", true => bool];
    msg![(); sv, "setAutohidesScrollers:", true => bool];
    msg![(); tv, "setHorizontallyResizable:", true => bool];
    msg![(); tv, "setAutoresizingMask:", 0usize => usize];
    let tc = msg![Id; tv, "textContainer"];
    msg![(); tc, "setWidthTracksTextView:", false => bool];
    msg![(); tc, "setContainerSize:", NSSize { w: 1e7, h: 1e7 } => NSSize];
}
unsafe fn segmented(parent: Id, labels: &[&str], r: NSRect, action: &str, mask: usize) -> Id {
    let ctl = *CONTROLLER.get().unwrap() as Id;
    let items: Vec<Id> = labels.iter().map(|l| ns(l)).collect();
    let s = msg![Id; cls("NSSegmentedControl"), "segmentedControlWithLabels:trackingMode:target:action:",
        ns_array(&items) => Id, 0usize => usize, ctl => Id, sel(action) => Sel];
    msg![(); s, "setFrame:", r => NSRect];
    msg![(); s, "setFont:", ui_font(13.0) => Id];
    msg![(); s, "setSelectedSegment:", 0isize => isize];
    add(parent, s, mask)
}
/// A checkbox that calls `action` on the controller when clicked.
unsafe fn toggle_checkbox(parent: Id, title: &str, r: NSRect, action: &str, mask: usize) -> Id {
    let ctl = *CONTROLLER.get().unwrap() as Id;
    let b = msg![Id; cls("NSButton"), "checkboxWithTitle:target:action:", ns(title) => Id, ctl => Id, sel(action) => Sel];
    msg![(); b, "setFrame:", r => NSRect];
    msg![(); b, "setFont:", ui_font(12.0) => Id];
    msg![(); b, "setState:", 0isize => isize];
    add(parent, b, mask)
}
unsafe fn checkbox(parent: Id, title: &str, r: NSRect, on: bool, mask: usize) -> Id {
    let b = msg![Id; cls("NSButton"), "checkboxWithTitle:target:action:", ns(title) => Id, NIL => Id, std::ptr::null::<c_void>() => Sel];
    msg![(); b, "setFrame:", r => NSRect];
    msg![(); b, "setFont:", ui_font(14.0) => Id];
    msg![(); b, "setState:", (on as isize) => isize];
    add(parent, b, mask)
}
unsafe fn plain_view(parent: Id, r: NSRect, mask: usize) -> Id {
    let v = msg![Id; msg![Id; cls("NSView"), "alloc"], "initWithFrame:", r => NSRect];
    add(parent, v, mask)
}
unsafe fn symbol(name: &str) -> Id {
    msg![Id; cls("NSImage"), "imageWithSystemSymbolName:accessibilityDescription:", ns(name) => Id, NIL => Id]
}

// ---------------------------------------------------------------- text drawing (inside drawRect)
unsafe fn draw_text(s: &str, x: f64, y: f64, font: Id, col: Id) {
    let keys = [ns("NSFont"), ns("NSColor")];
    let vals = [font, col];
    let attrs = msg![Id; cls("NSDictionary"), "dictionaryWithObjects:forKeys:count:", vals.as_ptr() => *const Id, keys.as_ptr() => *const Id, 2usize => usize];
    msg![(); ns(s), "drawAtPoint:withAttributes:", NSPoint { x, y } => NSPoint, attrs => Id];
}
/// white text on a soft dark halo (like heatmap sites' labels), smooth edges
unsafe fn draw_dark_halo(s: &str, x: f64, y: f64, font: Id) {
    let shadow = msg![Id; msg![Id; cls("NSShadow"), "alloc"], "init"];
    msg![(); shadow, "setShadowColor:", color(0.08, 0.08, 0.1, 1.0) => Id];
    msg![(); shadow, "setShadowBlurRadius:", 1.6f64 => f64];
    msg![(); shadow, "setShadowOffset:", NSSize { w: 0.0, h: 0.0 } => NSSize];
    let keys = [ns("NSFont"), ns("NSColor"), ns("NSShadow")];
    let vals = [font, color(1.0, 1.0, 1.0, 1.0), shadow];
    let attrs = msg![Id; cls("NSDictionary"), "dictionaryWithObjects:forKeys:count:", vals.as_ptr() => *const Id, keys.as_ptr() => *const Id, 3usize => usize];
    for _ in 0..3 {
        msg![(); ns(s), "drawAtPoint:withAttributes:", NSPoint { x, y } => NSPoint, attrs => Id];
    }
    msg![(); shadow, "release"];
}
/// rounded panel in the app's own (light/dark aware) colors
unsafe fn draw_panel(r: NSRect) {
    let path = msg![Id; cls("NSBezierPath"), "bezierPathWithRoundedRect:xRadius:yRadius:", r => NSRect, 7.0f64 => f64, 7.0f64 => f64];
    msg![(); sys_color("windowBackgroundColor"), "setFill"];
    msg![(); path, "fill"];
    msg![(); sys_color("separatorColor"), "setStroke"];
    msg![(); path, "setLineWidth:", 1.0f64 => f64];
    msg![(); path, "stroke"];
}
fn overlaps(a: NSRect, b: NSRect) -> bool {
    a.x < b.x + b.w && b.x < a.x + a.w && a.y < b.y + b.h && b.y < a.y + a.h
}

unsafe fn text_width(s: &str, font: Id) -> f64 {
    let keys = [ns("NSFont")];
    let vals = [font];
    let attrs = msg![Id; cls("NSDictionary"), "dictionaryWithObjects:forKeys:count:", vals.as_ptr() => *const Id, keys.as_ptr() => *const Id, 1usize => usize];
    msg![NSSize; ns(s), "sizeWithAttributes:", attrs => Id].w
}
unsafe fn current_cg() -> CG {
    let gc = msg![Id; cls("NSGraphicsContext"), "currentContext"];
    msg![CG; gc, "CGContext"]
}
unsafe fn view_bounds(v: Id) -> NSRect {
    msg![NSRect; v, "bounds"]
}
unsafe fn event_point(view: Id, event: Id) -> NSPoint {
    let p = msg![NSPoint; event, "locationInWindow"];
    msg![NSPoint; view, "convertPoint:fromView:", p => NSPoint, NIL => Id]
}
fn rgb(c: [u8; 3]) -> (f64, f64, f64) {
    (c[0] as f64 / 255.0, c[1] as f64 / 255.0, c[2] as f64 / 255.0)
}

/// The heatmap's own colour at a map point (the raster blended over the land colour),
/// so a pin can match what's around it.
fn heat_rgb_at(h: &Heat, mx: f64, my: f64) -> (f64, f64, f64) {
    let land = (0.925, 0.918, 0.894);
    let col = ((mx / geo::MAP_W) * h.w as f64).floor();
    let row = ((my - h.top) / (h.bottom - h.top) * h.h as f64).floor();
    if col < 0.0 || row < 0.0 || col >= h.w as f64 || row >= h.h as f64 {
        return land;
    }
    let o = (row as usize * h.w + col as usize) * 4;
    let px = &h.rgba[o..o + 4];
    let a = px[3] as f64 / 255.0;
    let ch = |i: usize, l: f64| px[i] as f64 / 255.0 + (1.0 - a) * l; // premultiplied over land
    (ch(0, land.0), ch(1, land.1), ch(2, land.2))
}

/// Pin in the colour of the heatmap around it, with a slightly darker rim of the same colour.
unsafe fn draw_pin(c: CG, x: f64, y: f64, zoomed: f64, est: bool, col: (f64, f64, f64)) {
    let r = (1.6 * zoomed.powf(0.5)).clamp(1.6, 4.0);
    let d = 0.72; // rim darkness
    CGContextBeginPath(c);
    CGContextAddEllipseInRect(c, rect(x - r, y - r, 2.0 * r, 2.0 * r));
    CGContextSetRGBStrokeColor(c, col.0 * d, col.1 * d, col.2 * d, 1.0);
    CGContextSetLineWidth(c, if est { 1.0 } else { 0.8 });
    if est {
        CGContextDrawPath(c, 2); // estimated: rim only
    } else {
        CGContextSetRGBFillColor(c, col.0, col.1, col.2, 1.0);
        CGContextDrawPath(c, 3);
    }
}

// ================================================================ map view (custom NSView subclass)
extern "C" fn map_is_flipped(_this: Id, _cmd: Sel) -> bool {
    true
}
extern "C" fn yes(_this: Id, _cmd: Sel) -> bool {
    true
}

unsafe fn heat_image(mode: usize, heat: &Heat) -> CG {
    let mut imgs = HEAT_IMG.lock().unwrap();
    if imgs[mode] == 0 {
        let cs = CGColorSpaceCreateDeviceRGB();
        let mut buf = heat.rgba.clone();
        let ctx = CGBitmapContextCreate(buf.as_mut_ptr() as *mut c_void, heat.w, heat.h, 8, heat.w * 4, cs, 1);
        let img = CGBitmapContextCreateImage(ctx); // copies the pixels
        CGContextRelease(ctx);
        CGColorSpaceRelease(cs);
        imgs[mode] = img as usize;
    }
    imgs[mode] as CG
}

fn fit_view(b: NSRect, v: &mut MapView) {
    // start on Europe to Central Asia
    let (x0, x1) = (geo::merc_x(-15.0), geo::merc_x(85.0));
    let (y0, y1) = (geo::merc_y(65.0), geo::merc_y(15.0));
    let k = (b.w / (x1 - x0)).min(b.h / (y1 - y0));
    v.k = k;
    v.k0 = k;
    v.tx = b.w / 2.0 - (x0 + x1) / 2.0 * k;
    v.ty = b.h / 2.0 - (y0 + y1) / 2.0 * k;
    v.init = true;
}

extern "C" fn map_draw(this: Id, _cmd: Sel, _dirty: NSRect) {
    unsafe {
        let c = current_cg();
        let b = view_bounds(this);
        CGContextSetRGBFillColor(c, 0.863, 0.910, 0.949, 1.0);
        CGContextFillRect(c, b);
        let small = mono(11.0);
        let text_col = color(0.15, 0.13, 0.36, 1.0);

        let world = match WORLD.get() {
            Some(Ok(w)) => w,
            Some(Err(_)) => {
                draw_text("Map data not installed yet — add world.g25map and places.tsv (see README).", 20.0, 20.0, ui_font(14.0), text_col);
                return;
            }
            None => {
                draw_text("Loading map…", 20.0, 20.0, ui_font(14.0), text_col);
                return;
            }
        };
        let (mut v, mode, hover) = with_state(|s| {
            if !s.view.init {
                fit_view(b, &mut s.view);
            }
            (s.view, s.map_mode, s.hover)
        });
        if !v.init {
            fit_view(b, &mut v);
        }

        CGContextSaveGState(c);
        CGContextTranslateCTM(c, v.tx, v.ty);
        CGContextScaleCTM(c, v.k, v.k);
        let land = *LAND_PATH.get_or_init(|| {
            let path = CGPathCreateMutable();
            for r in &world.rings {
                CGPathMoveToPoint(path, std::ptr::null(), r[0].0 as f64, r[0].1 as f64);
                for p in &r[1..] {
                    CGPathAddLineToPoint(path, std::ptr::null(), p.0 as f64, p.1 as f64);
                }
                CGPathCloseSubpath(path);
            }
            path as usize
        }) as CG;
        let land_path = |c: CG| {
            CGContextBeginPath(c);
            CGContextAddPath(c, land);
        };
        // land base
        CGContextSetRGBFillColor(c, 0.925, 0.918, 0.894, 1.0);
        land_path(c);
        CGContextFillPath(c);
        // heat, clipped to land
        let has_heat = with_state(|s| s.heat[mode].is_some());
        if has_heat {
            let st = STATE.lock().unwrap();
            if let Some(Some(h)) = st.as_ref().map(|s| &s.heat[mode]) {
                let img = heat_image(mode, h);
                CGContextSaveGState(c);
                land_path(c);
                CGContextClip(c);
                CGContextSetInterpolationQuality(c, 3);
                CGContextTranslateCTM(c, 0.0, h.bottom);
                CGContextScaleCTM(c, 1.0, -1.0);
                CGContextDrawImage(c, rect(0.0, 0.0, geo::MAP_W, h.bottom - h.top), img);
                CGContextRestoreGState(c);
            }
        }
        // borders and coastlines
        CGContextSetRGBStrokeColor(c, 0.17, 0.17, 0.16, 0.6);
        CGContextSetLineWidth(c, 0.7 / v.k);
        land_path(c);
        CGContextStrokePath(c);
        CGContextRestoreGState(c);

        // population dots + labels (screen space), unless turned off
        let st = STATE.lock().unwrap();
        let Some(s) = st.as_ref() else { return };
        if let Some(h) = s.heat[mode].as_ref().filter(|_| !s.hide_dots) {
            let zoomed = v.k / v.k0;
            // labels: bold white on a soft dark glow
            let label_font = msg![Id; cls("NSFont"), "systemFontOfSize:weight:", 10.5f64 => f64, 0.3f64 => f64];
            let mut placed: Vec<NSRect> = vec![];
            let mut dots: Vec<(f64, f64)> = vec![];
            for p in h.points.iter() {
                let (x, y) = (p.x * v.k + v.tx, p.y * v.k + v.ty);
                if x < -10.0 || y < -10.0 || x > b.w + 10.0 || y > b.h + 10.0 {
                    continue;
                }
                dots.push((x, y));
                draw_pin(c, x, y, zoomed, p.name.ends_with("(est.)"), heat_rgb_at(h, p.x, p.y));
            }
            // labels: closest first, more as you zoom in; each tries 4 spots and is skipped if all collide
            let budget = if zoomed > 4.0 { usize::MAX } else if zoomed > 2.2 { 60 } else { 30 };
            let mut shown = 0;
            for p in h.points.iter() {
                if shown >= budget {
                    break;
                }
                let (x, y) = (p.x * v.k + v.tx, p.y * v.k + v.ty);
                if x < 0.0 || y < 0.0 || x > b.w || y > b.h {
                    continue;
                }
                let tw = {
                    let mut cache = LABEL_W.lock().unwrap();
                    let m = cache.get_or_insert_with(HashMap::new);
                    match m.get(&p.name) {
                        Some(&w) => w,
                        None => {
                            let w = text_width(&p.name, label_font);
                            m.insert(p.name.clone(), w);
                            w
                        }
                    }
                };
                let (tw, th) = (tw + 2.0, 13.0);
                let spots = [(x + 5.0, y - th / 2.0), (x - 5.0 - tw, y - th / 2.0), (x - tw / 2.0, y - th - 4.0), (x - tw / 2.0, y + 4.0)];
                for (lx, ly) in spots {
                    let r = rect(lx, ly, tw, th);
                    let inside = lx >= 2.0 && ly >= 2.0 && lx + tw <= b.w - 2.0 && ly + th <= b.h - 2.0;
                    let hits_dot = dots.iter().any(|&(dx, dy)| overlaps(r, rect(dx - 2.5, dy - 2.5, 5.0, 5.0)));
                    if inside && !hits_dot && !placed.iter().any(|q| overlaps(r, *q)) {
                        draw_dark_halo(&p.name, lx + 1.0, ly, label_font);
                        placed.push(r);
                        shown += 1;
                        break;
                    }
                }
            }
            // hover tooltip
            if let Some(i) = hover {
                if let Some(p) = h.points.get(i) {
                    let (x, y) = (p.x * v.k + v.tx, p.y * v.k + v.ty);
                    let t = format!("{} · {:.4}  (#{})", p.name, p.dist, i + 1);
                    let tw = text_width(&t, small) + 14.0;
                    let bx = (x + 10.0).min(b.w - tw - 4.0);
                    draw_panel(rect(bx, y - 30.0, tw, 22.0));
                    draw_text(&t, bx + 7.0, y - 26.0, small, sys_color("labelColor"));
                }
            }
        } else if s.heat[mode].is_none() {
            draw_text(&s.heat_note, 20.0, 20.0, ui_font(14.0), text_col);
        }
        // legend (12 steps)
        let (lx, ly) = (12.0, b.h - 46.0);
        draw_panel(rect(lx, ly, 12.0 * 11.0 + 16.0 + 92.0, 36.0));
        // pin key, drawn like the map's pins: filled = official G25, rim only = estimated layer
        let kx = lx + 12.0 * 11.0 + 22.0;
        let (pr, pg, pb) = rgb(geo::RAMP[3]);
        draw_pin(c, kx + 3.0, ly + 10.0, 1.6, false, (pr, pg, pb));
        draw_pin(c, kx + 3.0, ly + 24.0, 1.6, true, (pr, pg, pb));
        draw_text("G25", kx + 11.0, ly + 3.0, mono(10.0), sys_color("secondaryLabelColor"));
        draw_text("estimated", kx + 11.0, ly + 17.0, mono(10.0), sys_color("secondaryLabelColor"));
        for (i, col) in geo::RAMP.iter().enumerate() {
            let (r, g, bb) = rgb(*col);
            CGContextSetRGBFillColor(c, r, g, bb, 1.0);
            CGContextFillRect(c, rect(lx + 8.0 + i as f64 * 11.0, ly + 6.0, 10.0, 8.0));
        }
        let lab = sys_color("secondaryLabelColor");
        draw_text("Closest", lx + 8.0, ly + 17.0, mono(10.0), lab);
        draw_text("Distant", lx + 12.0 * 11.0 - 32.0, ly + 17.0, mono(10.0), lab);
    }
}

unsafe fn redraw_map() {
    if let Some(w) = W.get() {
        msg![(); w.map as Id, "setNeedsDisplay:", true => bool];
    }
}

fn zoom_at(p: NSPoint, factor: f64) {
    with_state(|s| {
        let v = &mut s.view;
        if !v.init {
            return;
        }
        let k = (v.k * factor).clamp(v.k0 * 0.4, v.k0 * 40.0);
        let f = k / v.k;
        v.tx = p.x - (p.x - v.tx) * f;
        v.ty = p.y - (p.y - v.ty) * f;
        v.k = k;
    });
}

extern "C" fn map_mouse_down(this: Id, _cmd: Sel, e: Id) {
    let p = unsafe { event_point(this, e) };
    with_state(|s| s.view.last = p);
}
extern "C" fn map_mouse_dragged(this: Id, _cmd: Sel, e: Id) {
    let p = unsafe { event_point(this, e) };
    with_state(|s| {
        s.view.tx += p.x - s.view.last.x;
        s.view.ty += p.y - s.view.last.y;
        s.view.last = p;
        s.hover = None;
    });
    unsafe { redraw_map() };
}
extern "C" fn map_scroll(this: Id, _cmd: Sel, e: Id) {
    unsafe {
        let (dx, dy) = (msg![f64; e, "scrollingDeltaX"], msg![f64; e, "scrollingDeltaY"]);
        if msg![bool; e, "hasPreciseScrollingDeltas"] {
            with_state(|s| {
                s.view.tx += dx;
                s.view.ty += dy;
            });
        } else {
            zoom_at(event_point(this, e), 1.12f64.powf(dy));
        }
        redraw_map();
    }
}
extern "C" fn map_magnify(this: Id, _cmd: Sel, e: Id) {
    unsafe {
        let m = msg![f64; e, "magnification"];
        zoom_at(event_point(this, e), 1.0 + m);
        redraw_map();
    }
}
extern "C" fn map_mouse_moved(this: Id, _cmd: Sel, e: Id) {
    let p = unsafe { event_point(this, e) };
    let changed = with_state(|s| {
        let v = s.view;
        let mut best = None;
        if let Some(h) = &s.heat[s.map_mode] {
            let mut bd = 9.0;
            for (i, pt) in h.points.iter().enumerate() {
                let d = ((pt.x * v.k + v.tx - p.x).powi(2) + (pt.y * v.k + v.ty - p.y).powi(2)).sqrt();
                if d < bd {
                    bd = d;
                    best = Some(i);
                }
            }
        }
        let ch = best != s.hover;
        s.hover = best;
        ch
    });
    if changed {
        unsafe { redraw_map() };
    }
}
extern "C" fn map_mouse_exited(_this: Id, _cmd: Sel, _e: Id) {
    with_state(|s| s.hover = None);
    unsafe { redraw_map() };
}

// ================================================================ bars view (modeling results)
extern "C" fn bars_draw(this: Id, _cmd: Sel, _dirty: NSRect) {
    unsafe {
        let c = current_cg();
        let b = view_bounds(this);
        let (res, err, busy) = with_state(|s| (s.model_result.clone(), s.model_error.clone(), s.model_busy));
        let prim = sys_color("labelColor");
        let sec = sys_color("secondaryLabelColor");
        if busy {
            draw_text("Running…", 16.0, 14.0, ui_font(15.0), sec);
            return;
        }
        if let Some(e) = err {
            draw_text(&e, 16.0, 14.0, ui_font(14.0), sys_color("systemRedColor"));
            return;
        }
        let Some(r) = res else {
            draw_text("Paste or load your sources, then press Run.", 16.0, 14.0, ui_font(14.0), sec);
            return;
        };
        draw_text(&format!("Target: {}", r.target), 16.0, 10.0, ui_font(13.0), sec);
        let rp = if r.npop > 0 { format!("  ·  R{}P", r.npop) } else { String::new() };
        draw_text(&format!("Distance {:.4}%{rp}", r.distance * 100.0), 16.0, 28.0, ui_font(18.0), prim);
        // fit badge
        let (badge, bg, fg) = if r.distance < 0.02 {
            ("Good fit", (0.918, 0.953, 0.871), (0.153, 0.314, 0.039))
        } else if r.distance < 0.05 {
            ("OK fit", (0.980, 0.933, 0.855), (0.388, 0.220, 0.024))
        } else {
            ("Poor fit", (0.988, 0.922, 0.922), (0.475, 0.122, 0.122))
        };
        let bw = text_width(badge, ui_font(12.0)) + 18.0;
        CGContextSetRGBFillColor(c, bg.0, bg.1, bg.2, 1.0);
        CGContextFillRect(c, rect(b.w - bw - 16.0, 16.0, bw, 22.0));
        draw_text(badge, b.w - bw - 7.0, 19.0, ui_font(12.0), color(fg.0, fg.1, fg.2, 1.0));

        let mut y = 62.0;
        let bar_w = b.w - 16.0 - 70.0 - 16.0;
        for (name, v) in r.rows.iter() {
            if y > b.h - 40.0 {
                break;
            }
            draw_text(&format!("{:>5.1}", v * 100.0), 16.0, y + 2.0, mono(13.0), prim);
            draw_text(name, 70.0, y - 2.0, ui_font(12.0), prim);
            CGContextSetRGBFillColor(c, 0.5, 0.5, 0.5, 0.15);
            CGContextFillRect(c, rect(70.0, y + 15.0, bar_w, 6.0));
            CGContextSetRGBFillColor(c, 0.937, 0.624, 0.153, 1.0);
            CGContextFillRect(c, rect(70.0, y + 15.0, bar_w * v.clamp(0.0, 1.0), 6.0));
            y += 34.0;
        }
        draw_text(&r.footer, 16.0, b.h - 24.0, mono(11.0), sec);
    }
}

// ================================================================ distance view (modeling, Distance mode)
const DIST_TOP: f64 = 40.0;
const DIST_ROW: f64 = 26.0;

extern "C" fn dist_draw(this: Id, _cmd: Sel, _dirty: NSRect) {
    unsafe {
        let c = current_cg();
        let b = view_bounds(this);
        let (res, err, busy) = with_state(|s| (s.dist_result.clone(), s.model_error.clone(), s.model_busy));
        let prim = sys_color("labelColor");
        let sec = sys_color("secondaryLabelColor");
        if busy {
            draw_text("Running…", 16.0, 14.0, ui_font(15.0), sec);
            return;
        }
        if let Some(e) = err {
            draw_text(&e, 16.0, 14.0, ui_font(14.0), sys_color("systemRedColor"));
            return;
        }
        let Some(r) = res else {
            draw_text("Paste or load your sources, then press Run.", 16.0, 14.0, ui_font(14.0), sec);
            return;
        };
        draw_text(
            &format!("Distance to: {}  ·  {} populations ({})  ·  closest first", r.target, r.total, r.pool),
            16.0,
            12.0,
            ui_font(12.0),
            sec,
        );
        let manual = with_state(|s| s.dist_manual.then_some(s.dist_span));
        let (lo, span) = model::gradient_span(&r.rows, manual);
        let num_font = msg![Id; cls("NSFont"), "monospacedSystemFontOfSize:weight:", 12.0f64 => f64, 0.23f64 => f64];
        let rank_font = mono(11.0);
        let name_font = ui_font(13.0);
        let tert = sys_color("tertiaryLabelColor");
        let (cell_x, cell_w, cell_h) = (52.0, 78.0, 20.0);
        for (i, (name, v)) in r.rows.iter().enumerate() {
            let y = DIST_TOP + i as f64 * DIST_ROW;
            if i > 0 {
                CGContextSetRGBFillColor(c, 0.5, 0.5, 0.5, 0.15);
                CGContextFillRect(c, rect(14.0, y - 1.0, b.w - 28.0, 1.0));
            }
            let rank = format!("{}", i + 1);
            draw_text(&rank, 42.0 - text_width(&rank, rank_font), y + 5.0, rank_font, tert);
            let t = if span > 0.0 { ((v - lo) / span).min(1.0) } else { 0.0 };
            let (bg, fg) = model::distance_colors(t);
            let cell = rect(cell_x, y + 2.0, cell_w, cell_h);
            let path = msg![Id; cls("NSBezierPath"), "bezierPathWithRoundedRect:xRadius:yRadius:", cell => NSRect, 4.0f64 => f64, 4.0f64 => f64];
            msg![(); color(bg[0], bg[1], bg[2], 1.0), "setFill"];
            msg![(); path, "fill"];
            let num = format!("{v:.4}");
            let nw = text_width(&num, num_font);
            draw_text(&num, cell_x + (cell_w - nw) / 2.0, y + 4.0, num_font, color(fg[0], fg[1], fg[2], 1.0));
            draw_text(name, cell_x + cell_w + 14.0, y + 3.0, name_font, prim);
        }
        // legend
        let y = DIST_TOP + r.rows.len() as f64 * DIST_ROW + 12.0;
        let small = ui_font(11.0);
        let (left, right) = (format!("{lo:.3}"), format!("{:.3}{}", lo + span, if manual.is_some() { "+" } else { "" }));
        let (left, right) = (left.as_str(), right.as_str());
        draw_text(left, 16.0, y, small, sec);
        let bx = 16.0 + text_width(left, small) + 8.0;
        let steps = 40;
        let bw = 140.0;
        for k in 0..steps {
            let (col, _) = model::distance_colors(k as f64 / (steps - 1) as f64);
            CGContextSetRGBFillColor(c, col[0], col[1], col[2], 1.0);
            CGContextFillRect(c, rect(bx + k as f64 * bw / steps as f64, y + 4.0, bw / steps as f64 + 0.5, 8.0));
        }
        draw_text(right, bx + bw + 8.0, y, small, sec);
    }
}

/// Height the distance list needs for `n` rows (header, rows, legend).
fn dist_height(n: usize) -> f64 {
    DIST_TOP + n as f64 * DIST_ROW + 44.0
}

// ================================================================ actions
extern "C" fn on_nav(_this: Id, _cmd: Sel, sender: Id) {
    let tag = unsafe { msg![isize; sender, "tag"] } as usize;
    with_state(|s| s.page = tag.min(PAGES - 1));
    unsafe { apply_layout() };
}

extern "C" fn on_toggle_sidebar(_this: Id, _cmd: Sel, _sender: Id) {
    with_state(|s| s.collapsed = !s.collapsed);
    unsafe { apply_layout() };
}

unsafe fn apply_layout() {
    let Some(w) = W.get() else { return };
    let (page, collapsed) = with_state(|s| (s.page, s.collapsed));
    let sw = if collapsed { SIDEBAR_COLLAPSED } else { SIDEBAR_W };
    msg![(); w.sidebar as Id, "setFrame:", rect(0.0, 0.0, sw, WIN_H) => NSRect];
    msg![(); w.content as Id, "setFrame:", rect(sw, 0.0, WIN_W - sw, WIN_H) => NSRect];
    msg![(); w.side_title as Id, "setHidden:", collapsed => bool];
    if let Some(&t) = TOG.get() {
        let x = if collapsed { (SIDEBAR_COLLAPSED - 34.0) / 2.0 } else { SIDEBAR_W - 46.0 };
        msg![(); t as Id, "setFrame:", rect(x, WIN_H - 48.0, 34.0, 28.0) => NSRect];
    }
    msg![(); w.side_footer as Id, "setHidden:", collapsed => bool];
    for (i, &b) in w.nav.iter().enumerate() {
        let b = b as Id;
        msg![(); b, "setFrame:", rect(10.0, WIN_H - 92.0 - i as f64 * 40.0, sw - 20.0, 34.0) => NSRect];
        let title = if collapsed { String::new() } else { format!("  {}", NAV[i].0) };
        msg![(); b, "setTitle:", ns(&title) => Id];
        msg![(); b, "setImagePosition:", (if collapsed { 1usize } else { 2usize }) => usize]; // only / left
        let bg = if i == page { sys_color("quaternaryLabelColor") } else { NIL };
        set_layer_bg(b, bg, 7.0);
    }
    for (i, &p) in w.pages.iter().enumerate() {
        msg![(); p as Id, "setHidden:", (i != page) => bool];
    }
    if page == 1 {
        redraw_map();
    }
}

extern "C" fn on_choose(_this: Id, _cmd: Sel, _sender: Id) {
    unsafe {
        let panel = msg![Id; cls("NSOpenPanel"), "openPanel"];
        msg![(); panel, "setCanChooseFiles:", true => bool];
        msg![(); panel, "setCanChooseDirectories:", false => bool];
        msg![(); panel, "setAllowsMultipleSelection:", false => bool];
        msg![(); panel, "setMessage:", ns("Choose your raw DNA file (.txt, .csv, .gz or .zip)") => Id];
        if msg![isize; panel, "runModal"] == 1 {
            let url = msg![Id; msg![Id; panel, "URLs"], "firstObject"];
            let path = from_ns(msg![Id; url, "path"]);
            with_state(|s| {
                s.file = Some(path.clone());
                s.status = "Reading file…".into();
            });
            refresh_now();
            // health report needs only the raw file, so build it right away
            std::thread::spawn(move || {
                match raw::read_raw_file(&path) {
                    Ok(g) => {
                        let rep = health::report(&g);
                        let items = health::items(&g);
                        let tr = traits::predict(&g);
                        *RAWG.lock().unwrap() = Some(Arc::new(g));
                        qpadm::forget(qpadm::MY_DNA);
                        with_state(|s| {
                            s.health_text = Some(rep);
                            s.health_items = Some(items);
                            s.traits = Some(tr);
                            s.status = "Ready — press Convert".into();
                        });
                    }
                    Err(e) => with_state(|s| s.status = format!("Can't read this file: {e}")),
                }
                request_refresh();
            });
        }
    }
}

extern "C" fn on_convert(_this: Id, _cmd: Sel, _sender: Id) {
    let w = W.get().unwrap();
    let name = from_ns(unsafe { msg![Id; w.name as Id, "stringValue"] }).trim().replace([',', '\n', ':'], " ");
    let name = if name.is_empty() { "me".to_string() } else { name };
    let Some(file) = with_state(|s| s.file.clone()) else {
        with_state(|s| s.status = "Choose a raw DNA file first".into());
        refresh_now();
        return;
    };
    with_state(|s| {
        s.busy = true;
        s.name = name.clone();
        s.status = "Starting…".into();
    });
    refresh_now();
    std::thread::spawn(move || {
        let progress = |m: &str| {
            with_state(|s| s.status = m.to_string());
            request_refresh();
        };
        let result: Result<Report, String> = (|| {
            let g = match RAWG.lock().unwrap().clone() {
                Some(g) => g,
                None => {
                    progress("Reading your DNA file…");
                    Arc::new(raw::read_raw_file(&file)?)
                }
            };
            progress("Loading reference pack…");
            let pack = PACK.get_or_init(|| Pack::open(&resource("reference.g25pack"))).as_ref().map_err(|e| e.clone())?;
            engine::convert(pack, &g, &name, &engine::Settings::default(), &progress)
        })();
        let ok = result.is_ok();
        with_state(|s| {
            s.busy = false;
            match result {
                Ok(r) => {
                    s.line = Some(r.g25_line());
                    s.coords = Some(r.coords.to_vec());
                    s.conv_text = Some(format!("{}\n\n{}", r.g25_line(), r.summary()));
                    s.status = "Done".into();
                }
                Err(e) => {
                    s.status = "Failed".into();
                    s.conv_text = Some(format!("Error: {e}"));
                }
            }
        });
        if ok {
            start_heat();
        }
        request_refresh();
    });
}

extern "C" fn on_copy(_this: Id, _cmd: Sel, _sender: Id) {
    if let Some(line) = with_state(|s| s.line.clone()) {
        copy_to_clipboard(&line);
        with_state(|s| s.status = "Coordinates copied".into());
        refresh_now();
    }
}

fn copy_to_clipboard(text: &str) {
    unsafe {
        let pb = msg![Id; cls("NSPasteboard"), "generalPasteboard"];
        msg![isize; pb, "clearContents"];
        msg![bool; pb, "setString:forType:", ns(text) => Id, ns("public.utf8-plain-text") => Id];
    }
}

// ---------------------------------------------------------------- heatmap
fn start_heat() {
    let Some(me) = with_state(|s| s.coords.clone()) else { return };
    let places = match PLACES.get_or_init(|| geo::load_places_with_extras(&resource("places.tsv"), &resource("extra_places.tsv"))) {
        Ok(p) => p,
        Err(_) => {
            with_state(|s| s.heat_note = "Population locations (places.tsv) not installed yet — see README.".into());
            return;
        }
    };
    with_state(|s| s.heat_note = "Drawing your heatmap…".into());
    request_refresh();
    // official G25 sheets plus the estimated layer, when the build produced one
    let extra = |f: &str| std::fs::read_to_string(resource(f)).unwrap_or_default();
    let modern = geo::compute(&me, &format!("{}\n{}", data::MODERN, extra("extra_modern.txt")), places);
    let ancient = geo::compute(&me, &format!("{}\n{}", data::ANCIENT, extra("extra_ancient.txt")), places);
    {
        let mut imgs = HEAT_IMG.lock().unwrap();
        for i in imgs.iter_mut() {
            if *i != 0 {
                unsafe { CGImageRelease(*i as CG) };
                *i = 0;
            }
        }
    }
    with_state(|s| {
        s.heat = [Some(modern), Some(ancient)];
        s.hover = None;
    });
    request_refresh();
}

extern "C" fn on_map_mode(_this: Id, _cmd: Sel, sender: Id) {
    let m = unsafe { msg![isize; sender, "selectedSegment"] }.max(0) as usize;
    with_state(|s| {
        s.map_mode = m.min(1);
        s.hover = None;
    });
    refresh_now();
}
extern "C" fn on_zoom_in(_this: Id, _cmd: Sel, _sender: Id) {
    zoom_center(1.5);
}
/// Heatmap dots on/off (button under the zoom buttons).
extern "C" fn on_toggle_dots(_this: Id, _cmd: Sel, sender: Id) {
    let hide = with_state(|s| {
        s.hide_dots = !s.hide_dots;
        s.hide_dots
    });
    unsafe {
        set_dots_button(sender, hide);
        redraw_map();
    }
}
unsafe fn set_dots_button(b: Id, hidden: bool) {
    msg![(); b, "setImage:", symbol(if hidden { "mappin.slash" } else { "mappin.and.ellipse" }) => Id];
    msg![(); b, "setToolTip:", ns(if hidden { "Show population dots" } else { "Hide population dots" }) => Id];
}

extern "C" fn on_zoom_out(_this: Id, _cmd: Sel, _sender: Id) {
    zoom_center(1.0 / 1.5);
}
fn zoom_center(f: f64) {
    unsafe {
        let Some(w) = W.get() else { return };
        let b = view_bounds(w.map as Id);
        zoom_at(NSPoint { x: b.w / 2.0, y: b.h / 2.0 }, f);
        redraw_map();
    }
}

// ---------------------------------------------------------------- modeling
fn set_sources(text: String, label: String, builtin: isize) {
    with_state(|s| {
        s.sources = text;
        s.sources_label = label;
        s.builtin = builtin;
    });
    refresh_now();
}

extern "C" fn on_builtin(_this: Id, _cmd: Sel, sender: Id) {
    let i = unsafe { msg![isize; sender, "selectedSegment"] };
    if i < 0 {
        return;
    }
    set_sources(builtin_text(i), builtin_label(i), i);
}

/// `builtin` index for the Modern/Ancient sets (0..5 are the IllustrativeDNA eras)
const G25_BASE: isize = 6;

extern "C" fn on_builtin_g25(_this: Id, _cmd: Sel, sender: Id) {
    let i = unsafe { msg![isize; sender, "selectedSegment"] };
    if i < 0 {
        return;
    }
    set_sources(builtin_text(G25_BASE + i), builtin_label(G25_BASE + i), G25_BASE + i);
}

/// Source text for a `builtin` index (0..5 eras, 6/7 Modern/Ancient).
fn builtin_text(i: isize) -> String {
    if i >= G25_BASE { data::g25_set((i - G25_BASE) as usize).to_string() } else { data::builtin(i as usize) }
}
fn builtin_label(i: isize) -> String {
    let n = data::count_lines(&builtin_text(i));
    if i >= G25_BASE {
        format!("Built-in: {} G25 averages · {n} populations", data::G25_SETS[(i - G25_BASE) as usize])
    } else {
        format!("Built-in: {} · {n} populations", data::BUILTIN[i as usize])
    }
}

/// Show/hide the built-in pickers: Modern/Ancient by default, or the IllustrativeDNA eras instead.
fn show_builtin_pickers() {
    let (on, illus) = with_state(|s| (s.show_builtin, s.show_illus));
    unsafe {
        if let Some(w) = W.get() {
            msg![(); w.illus_toggle as Id, "setHidden:", !on => bool];
            msg![(); w.builtin as Id, "setHidden:", !(on && illus) => bool];
            msg![(); w.builtin_g25 as Id, "setHidden:", !(on && !illus) => bool];
        }
    }
}

/// Drop the built-in set in use if its picker is no longer shown, and when the built-ins are
/// shown and nothing else is in use, pick the first set of the visible picker so a run
/// always has sources.
fn drop_hidden_builtin(s: &mut Shared) {
    let shown = s.show_builtin && if s.builtin >= G25_BASE { !s.show_illus } else { s.show_illus };
    if s.builtin >= 0 && !shown {
        s.sources.clear();
        s.sources_label = NO_SOURCES.into();
        s.builtin = -1;
    }
    if s.show_builtin && s.builtin < 0 && s.sources.trim().is_empty() {
        let i = if s.show_illus { 0 } else { G25_BASE };
        s.sources = builtin_text(i);
        s.sources_label = builtin_label(i);
        s.builtin = i;
    }
}

extern "C" fn on_toggle_illus(_this: Id, _cmd: Sel, sender: Id) {
    let on = unsafe { msg![isize; sender, "state"] } == 1;
    with_state(|s| {
        s.show_illus = on;
        drop_hidden_builtin(s);
    });
    show_builtin_pickers();
    refresh_now();
}

extern "C" fn on_paste_sources(_this: Id, _cmd: Sel, _sender: Id) {
    let t = unsafe {
        let pb = msg![Id; cls("NSPasteboard"), "generalPasteboard"];
        from_ns(msg![Id; pb, "stringForType:", ns("public.utf8-plain-text") => Id])
    };
    match model::parse_sheet(&t) {
        Ok(sh) => set_sources(t, format!("Pasted · {} sources", sh.rows.len()), -1),
        Err(e) => with_state(|s| s.model_error = Some(format!("Clipboard doesn't hold G25 coordinates ({e}).")))
    }
    refresh_now();
}

extern "C" fn on_load_sources(_this: Id, _cmd: Sel, _sender: Id) {
    unsafe {
        let panel = msg![Id; cls("NSOpenPanel"), "openPanel"];
        msg![(); panel, "setCanChooseFiles:", true => bool];
        msg![(); panel, "setAllowsMultipleSelection:", false => bool];
        msg![(); panel, "setMessage:", ns("Choose a G25 sheet (.txt or .csv)") => Id];
        if msg![isize; panel, "runModal"] == 1 {
            let url = msg![Id; msg![Id; panel, "URLs"], "firstObject"];
            let path = from_ns(msg![Id; url, "path"]);
            let fname = path.rsplit('/').next().unwrap_or(&path).to_string();
            match std::fs::read_to_string(&path).map_err(|e| e.to_string()).and_then(|t| model::parse_sheet(&t).map(|s| (t, s.rows.len()))) {
                Ok((t, n)) => set_sources(t, format!("{fname} · {n} sources"), -1),
                Err(e) => with_state(|s| s.model_error = Some(format!("Couldn't use {fname}: {e}"))),
            }
            refresh_now();
        }
    }
}

/// The R#P field: "3-5" gives (3, 5), "4" gives (4, 4), "0" or junk means all populations.
fn parse_npop(s: &str) -> (usize, usize) {
    let s = s.trim().to_ascii_uppercase();
    let s = s.trim_start_matches('R').trim_end_matches('P');
    let mut it = s.split(['-', '–']).map(|x| x.trim().parse::<usize>().ok());
    match (it.next().flatten(), it.next().flatten()) {
        (Some(a), Some(b)) if a > 0 && b >= a => (a, b.min(a + 6)),
        (Some(a), _) => (a, a),
        _ => (0, 0),
    }
}

/// A target pasted on the modeling page also drives the heatmap.
fn adopt_target(coords: &[f64]) {
    let changed = with_state(|s| {
        let ch = s.coords.as_deref() != Some(coords);
        if ch {
            s.coords = Some(coords.to_vec());
        }
        ch
    });
    if changed {
        std::thread::spawn(start_heat);
    }
}

extern "C" fn on_toggle_builtin(_this: Id, _cmd: Sel, sender: Id) {
    let on = unsafe { msg![isize; sender, "state"] } == 1;
    with_state(|s| {
        s.show_builtin = on;
        // hiding the built-ins also drops a built-in set that was in use
        drop_hidden_builtin(s);
    });
    show_builtin_pickers();
    unsafe {
        if W.get().is_some() {
            apply_model_mode(with_state(|s| s.dist_mode));
        }
    }
    refresh_now();
}

/// "Auto steepness": on = the gradient stretches from the closest to the furthest row shown;
/// off = the slider sets it. Turning it off starts the slider where auto left it, so nothing jumps.
extern "C" fn on_grad_auto(_this: Id, _cmd: Sel, sender: Id) {
    let auto = unsafe { msg![isize; sender, "state"] } == 1;
    with_state(|s| {
        if !auto {
            if let Some(r) = &s.dist_result {
                let span = model::gradient_span(&r.rows, None).1;
                if span > 0.0 {
                    s.dist_span = span.clamp(model::SPAN_STEEP, model::SPAN_GENTLE);
                }
            }
        }
        s.dist_manual = !auto;
    });
    refresh_now();
}

/// Steepness value typed in: the distance gap to full red (like a Vahaduo distance).
/// Typing a value switches to manual.
extern "C" fn on_grad_value(_this: Id, _cmd: Sel, sender: Id) {
    let t = from_ns(unsafe { msg![Id; sender, "stringValue"] });
    if let Ok(v) = t.trim().replace(',', ".").parse::<f64>() {
        if v > 0.0 {
            with_state(|s| {
                s.dist_span = v.clamp(model::SPAN_STEEP, model::SPAN_GENTLE);
                s.dist_manual = true;
            });
        }
    }
    refresh_now();
}

extern "C" fn on_grad_steep(_this: Id, _cmd: Sel, sender: Id) {
    let v = unsafe { msg![f64; sender, "doubleValue"] };
    with_state(|s| s.dist_span = model::span_for_steepness(v));
    refresh_now();
}

const NO_SOURCES: &str = "No sources yet — pick a built-in set, or paste or load your own";
const NEED_SOURCES: &str = "Add sources first: paste or load your own (or turn on built-in databases).";

extern "C" fn on_model_mode(_this: Id, _cmd: Sel, sender: Id) {
    let dist = unsafe { msg![isize; sender, "selectedSegment"] } == 1;
    with_state(|s| s.dist_mode = dist);
    unsafe { apply_model_mode(dist) };
    refresh_now();
}

/// Switching the comparison set re-runs straight away once there's a distance result on screen.
extern "C" fn on_dist_pool(this: Id, cmd: Sel, sender: Id) {
    if with_state(|s| s.dist_result.is_some() && !s.model_busy) {
        on_run_model(this, cmd, sender);
    }
}

unsafe fn apply_model_mode(dist: bool) {
    let Some(w) = W.get() else { return };
    for v in w.admix {
        msg![(); v as Id, "setHidden:", dist => bool];
    }
    for v in w.dist_ctl {
        msg![(); v as Id, "setHidden:", !dist => bool];
    }
    let builtin_on = with_state(|s| s.show_builtin);
    if !builtin_on {
        msg![(); w.dist_ctl[2] as Id, "setHidden:", true => bool];
    }
    // gradient controls sit right after the pool picker (or after "Show" when it's hidden)
    let x0 = if builtin_on { 418.0 } else { 146.0 };
    for (i, &b) in w.grad_ctl.iter().enumerate() {
        msg![(); b as Id, "setHidden:", !dist => bool];
        let mut f = msg![NSRect; b as Id, "frame"];
        f.x = x0 + [0.0, 116.0, 192.0][i];
        msg![(); b as Id, "setFrame:", f => NSRect];
    }
    msg![(); w.bars as Id, "setHidden:", dist => bool];
    msg![(); w.dist_scroll as Id, "setHidden:", !dist => bool];
}

fn run_distance(w: &Widgets) {
    let (tgt, show, pool) = unsafe {
        (
            from_ns(msg![Id; w.target as Id, "stringValue"]),
            from_ns(msg![Id; w.dist_ctl[1] as Id, "stringValue"]),
            if with_state(|s| s.show_builtin) { msg![isize; w.dist_ctl[2] as Id, "selectedSegment"].max(0) as usize } else { 3 },
        )
    };
    if pool == 3 && with_state(|s| s.sources.trim().is_empty()) {
        with_state(|s| s.model_error = Some(NEED_SOURCES.into()));
        return refresh_now();
    }
    let (src, src_label) = with_state(|s| {
        s.model_busy = true;
        s.model_error = None;
        (s.sources.clone(), s.sources_label.clone())
    });
    refresh_now();
    std::thread::spawn(move || {
        let res: Result<DistView, String> = (|| {
            let target = model::parse_sheet(&tgt).map_err(|_| "Target: paste a G25 line (Name,PC1,…,PC25) or convert your DNA file first".to_string())?;
            let (text, label) = match pool {
                0 => (format!("{}\n{}", data::MODERN, data::ANCIENT), "modern and ancient".to_string()),
                1 => (data::MODERN.to_string(), "modern".to_string()),
                2 => (data::ANCIENT.to_string(), "ancient".to_string()),
                _ => (src, src_label),
            };
            let sources = model::parse_sheet(&text).map_err(|e| format!("Sources: {e}"))?;
            let all = model::distances(&target.rows[0], &sources).map_err(|e| format!("Can't compare: {e}"))?;
            let n = show.trim().parse::<usize>().unwrap_or(25).clamp(1, 1000).min(all.len());
            adopt_target(&target.rows[0]);
            let rows = all[..n].to_vec();
            Ok(DistView {
                text: model::render_distances(&target.labels[0], &rows),
                target: target.labels[0].clone(),
                pool: label,
                total: all.len(),
                rows,
            })
        })();
        with_state(|s| {
            s.model_busy = false;
            match res {
                Ok(v) => {
                    s.history.push(v.text.clone());
                    s.dist_result = Some(v);
                }
                Err(e) => s.model_error = Some(e),
            }
        });
        request_refresh();
    });
}

extern "C" fn on_run_model(_this: Id, _cmd: Sel, _sender: Id) {
    let w = W.get().unwrap();
    if with_state(|s| s.dist_mode) {
        return run_distance(w);
    }
    let (tgt, npop, cycles, agg, adc) = unsafe {
        (
            from_ns(msg![Id; w.target as Id, "stringValue"]),
            from_ns(msg![Id; w.npop as Id, "stringValue"]),
            from_ns(msg![Id; w.cycles as Id, "stringValue"]),
            msg![isize; w.agg as Id, "state"] == 1,
            msg![isize; w.adc as Id, "state"] == 1,
        )
    };
    if with_state(|s| s.sources.trim().is_empty()) {
        with_state(|s| s.model_error = Some(NEED_SOURCES.into()));
        return refresh_now();
    }
    let src = with_state(|s| {
        s.model_busy = true;
        s.model_error = None;
        s.sources.clone()
    });
    refresh_now();
    std::thread::spawn(move || {
        // "3-5" = try 3, 4 and 5 populations and keep the best; "4" = exactly 4; "0" = all
        let (lo, hi) = parse_npop(&npop);
        let o = model::Options {
            npop: lo,
            npop_max: hi,
            cycles: cycles.trim().trim_end_matches(['x', '×']).parse().unwrap_or(1.0f64).max(0.01),
            aggregate: agg,
            adc: if adc { 0.25 } else { 0.0 },
            recalc: adc,
            ..Default::default()
        };
        let res: Result<ModelView, String> = (|| {
            let sources = model::parse_sheet(&src).map_err(|e| format!("Sources: {e}"))?;
            let target = model::parse_sheet(&tgt).map_err(|_| "Target: paste a G25 line (Name,PC1,…,PC25) or convert your DNA file first".to_string())?;
            let t0 = std::time::Instant::now();
            let fit = model::fit(&target.rows[0], &sources, &o)?;
            let footer = format!(
                "{} sources · {} {} · {:.2} s",
                fit.sources,
                fit.cycles_or_iters,
                if fit.npop == 0 { "cycles" } else { "iterations" },
                t0.elapsed().as_secs_f64()
            );
            adopt_target(&target.rows[0]);
            Ok(ModelView {
                target: target.labels[0].clone(),
                distance: fit.distance,
                npop: fit.npop,
                rows: fit.rows.clone(),
                footer,
                text: fit.render(&target.labels[0], &o),
            })
        })();
        with_state(|s| {
            s.model_busy = false;
            match res {
                Ok(v) => {
                    s.history.push(v.text.clone());
                    s.model_result = Some(v);
                }
                Err(e) => s.model_error = Some(e),
            }
        });
        request_refresh();
    });
}

extern "C" fn on_copy_model(_this: Id, _cmd: Sel, _sender: Id) {
    let t = with_state(|s| {
        if s.dist_mode {
            s.dist_result.as_ref().map(|r| r.text.clone())
        } else {
            s.model_result.as_ref().map(|r| r.text.clone())
        }
    });
    if let Some(t) = t {
        copy_to_clipboard(&t);
    }
}

extern "C" fn on_history(_this: Id, _cmd: Sel, _sender: Id) {
    unsafe {
        static HWIN: OnceLock<(usize, usize)> = OnceLock::new();
        let (win, tv) = *HWIN.get_or_init(|| {
            let win = msg![Id; cls("NSWindow"), "alloc"];
            let win = msg![Id; win, "initWithContentRect:styleMask:backing:defer:",
                rect(0.0, 0.0, 520.0, 560.0) => NSRect, 15usize => usize, 2usize => usize, false => bool];
            msg![(); win, "setTitle:", ns("Model history") => Id];
            msg![(); win, "setReleasedWhenClosed:", false => bool];
            let cv = msg![Id; win, "contentView"];
            let tv = text_area(cv, rect(0.0, 0.0, 520.0, 560.0), mono(12.0), M_W | M_H);
            (win as usize, tv as usize)
        });
        let text = with_state(|s| {
            if s.history.is_empty() {
                "No runs yet.".to_string()
            } else {
                s.history.iter().rev().cloned().collect::<Vec<_>>().join("\n────────────────────────\n\n")
            }
        });
        msg![(); tv as Id, "setString:", ns(&text) => Id];
        msg![(); win as Id, "center"];
        msg![(); win as Id, "makeKeyAndOrderFront:", NIL => Id];
    }
}

extern "C" fn on_refresh(_this: Id, _cmd: Sel, _arg: Id) {
    refresh_now();
}
extern "C" fn should_terminate_after_last_window(_this: Id, _cmd: Sel, _app: Id) -> bool {
    true
}

fn refresh_now() {
    let Some(w) = W.get() else { return };
    let snap = with_state(|s| {
        (
            s.file.clone(),
            s.status.clone(),
            s.conv_text.take(),
            s.busy,
            s.line.clone(),
            s.health_text.take(),
            s.sources_label.clone(),
            s.builtin,
            s.model_busy,
            s.name.clone(),
            {
                let fresh = s.line.clone().filter(|l| s.target_filled.as_ref() != Some(l));
                if fresh.is_some() {
                    s.target_filled = fresh.clone();
                }
                fresh
            },
            s.heat[s.map_mode].as_ref().map(|h| {
                h.points.iter().take(3).map(|p| format!("{} {:.3}", p.name, p.dist)).collect::<Vec<_>>().join("  ·  ")
            }),
        )
    });
    let (file, status, conv, busy, line, health, src_label, builtin, model_busy, name, fresh_target, closest) = snap;
    unsafe {
        let fname = file.as_deref().map(|f| f.rsplit('/').next().unwrap_or(f).to_string()).unwrap_or_else(|| "No file chosen".into());
        msg![(); w.file as Id, "setStringValue:", ns(&fname) => Id];
        msg![(); w.status as Id, "setStringValue:", ns(&status) => Id];
        msg![(); w.convert as Id, "setEnabled:", !busy => bool];
        msg![(); w.copy as Id, "setEnabled:", line.is_some() && !busy => bool];
        if let Some(t) = conv {
            msg![(); w.conv_text as Id, "setString:", ns(&t) => Id];
        }
        if let Some(l) = &fresh_target {
            msg![(); w.target as Id, "setStringValue:", ns(l) => Id];
        }
        if health.is_some() {
            size_health_view(w.health as Id);
        }
        msg![(); w.src_label as Id, "setStringValue:", ns(&src_label) => Id];
        msg![(); w.builtin as Id, "setSelectedSegment:", if (0..G25_BASE).contains(&builtin) { builtin } else { -1 } => isize];
        msg![(); w.builtin_g25 as Id, "setSelectedSegment:", if builtin >= G25_BASE { builtin - G25_BASE } else { -1 } => isize];
        msg![(); w.run as Id, "setEnabled:", !model_busy => bool];
        msg![(); w.bars as Id, "setNeedsDisplay:", true => bool];
        let rows = with_state(|s| s.dist_result.as_ref().map_or(0, |r| r.rows.len()));
        let vis = msg![NSSize; w.dist_scroll as Id, "contentSize"];
        msg![(); w.dist_view as Id, "setFrameSize:", NSSize { w: vis.w, h: dist_height(rows).max(vis.h) } => NSSize];
        msg![(); w.dist_view as Id, "setNeedsDisplay:", true => bool];
        // steepness slider: greyed while auto, and shows where auto currently sits
        let (manual, span) = with_state(|s| {
            let auto = s.dist_result.as_ref().map(|r| model::gradient_span(&r.rows, None).1);
            (s.dist_manual, if s.dist_manual { Some(s.dist_span) } else { auto })
        });
        let steep = w.grad_ctl[1] as Id;
        msg![(); steep, "setEnabled:", manual => bool];
        msg![(); w.grad_ctl[0] as Id, "setState:", !manual as isize => isize];
        if let Some(sp) = span {
            msg![(); steep, "setDoubleValue:", model::steepness_for_span(sp) => f64];
            // value box: the distance gap from the closest row to full red (leave it alone while typing)
            let val = w.grad_ctl[2] as Id;
            if msg![Id; val, "currentEditor"].is_null() {
                msg![(); val, "setStringValue:", ns(&format!("{sp:.3}")) => Id];
            }
        }
        msg![(); w.traits_view as Id, "setNeedsDisplay:", true => bool];
        if let Some(&(_, lv)) = LIKENESS.get() {
            msg![(); lv as Id, "setNeedsDisplay:", true => bool];
        }
        msg![(); w.side_footer as Id, "setStringValue:", ns(&format!("Sample: {name}")) => Id];
        let cl = closest.map(|c| format!("Closest: {c}")).unwrap_or_default();
        msg![(); w.closest as Id, "setStringValue:", ns(&cl) => Id];
        redraw_map();
    }
}

// ================================================================ setup
unsafe fn register_classes() -> Id {
    let v = CString::new("v@:@").unwrap();
    let b = CString::new("B@:@").unwrap();
    let b0 = CString::new("B@:").unwrap();
    let dr = CString::new("v@:{CGRect={CGPoint=dd}{CGSize=dd}}").unwrap();

    // controller
    let name = CString::new("G25Controller").unwrap();
    let c = objc_allocateClassPair(cls("NSObject"), name.as_ptr(), 0);
    for (s, f) in [
        ("nav:", on_nav as *const c_void),
        ("toggleSidebar:", on_toggle_sidebar as *const c_void),
        ("chooseFile:", on_choose as *const c_void),
        ("convert:", on_convert as *const c_void),
        ("copyCoords:", on_copy as *const c_void),
        ("mapMode:", on_map_mode as *const c_void),
        ("zoomIn:", on_zoom_in as *const c_void),
        ("zoomOut:", on_zoom_out as *const c_void),
        ("toggleDots:", on_toggle_dots as *const c_void),
        ("pickBuiltin:", on_builtin as *const c_void),
        ("pickBuiltinG25:", on_builtin_g25 as *const c_void),
        ("toggleIllus:", on_toggle_illus as *const c_void),
        ("pasteSources:", on_paste_sources as *const c_void),
        ("loadSources:", on_load_sources as *const c_void),
        ("runModel:", on_run_model as *const c_void),
        ("copyModel:", on_copy_model as *const c_void),
        ("modelMode:", on_model_mode as *const c_void),
        ("toggleBuiltin:", on_toggle_builtin as *const c_void),
        ("gradAuto:", on_grad_auto as *const c_void),
        ("gradSteep:", on_grad_steep as *const c_void),
        ("gradValue:", on_grad_value as *const c_void),
        ("distPool:", on_dist_pool as *const c_void),
        ("showHistory:", on_history as *const c_void),
        ("generateLikeness:", on_likeness as *const c_void),
        ("refresh:", on_refresh as *const c_void),
        ("qpAdd:", on_qp_add as *const c_void),
        ("qpPreset:", on_qp_preset as *const c_void),
        ("qpSuggest:", on_qp_suggest as *const c_void),
        ("qpCheck:", on_qp_check as *const c_void),
        ("qpRun:", on_qp_run as *const c_void),
        ("qpRotate:", on_qp_rotate as *const c_void),
        ("qpCancel:", on_qp_cancel as *const c_void),
        ("qpReset:", on_qp_reset as *const c_void),
        ("qpOwn:", on_qp_own as *const c_void),
        ("qpPie:", on_qp_pie as *const c_void),
        ("qpCopy:", on_qp_copy as *const c_void),
        ("qpExport:", on_qp_export as *const c_void),
        ("qpAdvanced:", on_qp_advanced as *const c_void),
        ("qpRefresh:", on_qp_refresh as *const c_void),
    ] {
        class_addMethod(c, sel(s), f, v.as_ptr());
    }
    #[cfg(feature = "shots")]
    for (s, f) in shots::methods() {
        class_addMethod(c, sel(s), f, v.as_ptr());
    }
    class_addMethod(c, sel("applicationShouldTerminateAfterLastWindowClosed:"), should_terminate_after_last_window as *const c_void, b.as_ptr());
    objc_registerClassPair(c);

    // map view
    let name = CString::new("G25MapView").unwrap();
    let m = objc_allocateClassPair(cls("NSView"), name.as_ptr(), 0);
    class_addMethod(m, sel("drawRect:"), map_draw as *const c_void, dr.as_ptr());
    class_addMethod(m, sel("isFlipped"), map_is_flipped as *const c_void, b0.as_ptr());
    class_addMethod(m, sel("acceptsFirstResponder"), yes as *const c_void, b0.as_ptr());
    class_addMethod(m, sel("acceptsFirstMouse:"), yes as *const c_void, b.as_ptr());
    for (s, f) in [
        ("mouseDown:", map_mouse_down as *const c_void),
        ("mouseDragged:", map_mouse_dragged as *const c_void),
        ("scrollWheel:", map_scroll as *const c_void),
        ("magnifyWithEvent:", map_magnify as *const c_void),
        ("mouseMoved:", map_mouse_moved as *const c_void),
        ("mouseExited:", map_mouse_exited as *const c_void),
    ] {
        class_addMethod(m, sel(s), f, v.as_ptr());
    }
    objc_registerClassPair(m);

    for (cname, draw) in [("G25TraitsView", traits_draw as *const c_void), ("G25LikenessView", likeness_draw as *const c_void), ("G25HealthView", health_draw as *const c_void), ("G25QpadmView", qp_draw as *const c_void)] {
        let n = CString::new(cname).unwrap();
        let k = objc_allocateClassPair(cls("NSView"), n.as_ptr(), 0);
        class_addMethod(k, sel("drawRect:"), draw, dr.as_ptr());
        class_addMethod(k, sel("isFlipped"), map_is_flipped as *const c_void, b0.as_ptr());
        objc_registerClassPair(k);
    }

    // bars view
    let name = CString::new("G25BarsView").unwrap();
    let bv = objc_allocateClassPair(cls("NSView"), name.as_ptr(), 0);
    class_addMethod(bv, sel("drawRect:"), bars_draw as *const c_void, dr.as_ptr());
    class_addMethod(bv, sel("isFlipped"), map_is_flipped as *const c_void, b0.as_ptr());
    objc_registerClassPair(bv);

    // distance list view (lives in a scroll view)
    let name = CString::new("G25DistView").unwrap();
    let dv = objc_allocateClassPair(cls("NSView"), name.as_ptr(), 0);
    class_addMethod(dv, sel("drawRect:"), dist_draw as *const c_void, dr.as_ptr());
    class_addMethod(dv, sel("isFlipped"), map_is_flipped as *const c_void, b0.as_ptr());
    objc_registerClassPair(dv);

    msg![Id; c, "new"]
}

unsafe fn menu_item(title: &str, action: &str, key: &str) -> Id {
    let alloc = msg![Id; cls("NSMenuItem"), "alloc"];
    msg![Id; alloc, "initWithTitle:action:keyEquivalent:", ns(title) => Id, sel(action) => Sel, ns(key) => Id]
}
unsafe fn build_menu(app: Id) {
    let bar = msg![Id; cls("NSMenu"), "new"];
    let app_item = msg![Id; cls("NSMenuItem"), "new"];
    let app_menu = msg![Id; cls("NSMenu"), "new"];
    msg![(); app_menu, "addItem:", menu_item("Quit genesloppa", "terminate:", "q") => Id];
    msg![(); app_item, "setSubmenu:", app_menu => Id];
    msg![(); bar, "addItem:", app_item => Id];
    let edit_item = msg![Id; cls("NSMenuItem"), "new"];
    let edit = msg![Id; msg![Id; cls("NSMenu"), "alloc"], "initWithTitle:", ns("Edit") => Id];
    for (t, a, k) in [("Undo", "undo:", "z"), ("Cut", "cut:", "x"), ("Copy", "copy:", "c"), ("Paste", "paste:", "v"), ("Select All", "selectAll:", "a")] {
        msg![(); edit, "addItem:", menu_item(t, a, k) => Id];
    }
    msg![(); edit_item, "setSubmenu:", edit => Id];
    msg![(); bar, "addItem:", edit_item => Id];
    // View > Toggle Sidebar, ctrl-cmd-S like other Mac apps
    let view_item = msg![Id; cls("NSMenuItem"), "new"];
    let view = msg![Id; msg![Id; cls("NSMenu"), "alloc"], "initWithTitle:", ns("View") => Id];
    let tog = menu_item("Toggle Sidebar", "toggleSidebar:", "s");
    msg![(); tog, "setKeyEquivalentModifierMask:", ((1usize << 18) | (1usize << 20)) => usize];
    msg![(); tog, "setTarget:", (*CONTROLLER.get().unwrap() as Id) => Id];
    msg![(); view, "addItem:", tog => Id];
    msg![(); view_item, "setSubmenu:", view => Id];
    msg![(); bar, "addItem:", view_item => Id];
    msg![(); app, "setMainMenu:", bar => Id];
}

unsafe fn page_title(p: Id, h: f64, text: &str) {
    label(p, text, place(h, 28.0, 22.0, 500.0, 40.0), 28.0, M_BOTTOM);
}

unsafe fn build_converter(p: Id, w: f64, h: f64) -> (Id, Id, Id, Id, Id, Id) {
    page_title(p, h, "Converter");
    label(p, "Raw DNA  →  Global25 coordinates", place(h, 30.0, 62.0, 500.0, 22.0), 15.0, M_BOTTOM);
    label(p, "Your name", place(h, 30.0, 106.0, 110.0, 24.0), 16.0, M_BOTTOM);
    let name = field(p, place(h, 146.0, 104.0, 260.0, 26.0), "me", ui_font(15.0), M_BOTTOM);
    button(p, "Choose DNA file…", place(h, 24.0, 148.0, 210.0, 34.0), "chooseFile:", M_BOTTOM);
    let file = label(p, "No file chosen", place(h, 250.0, 154.0, w - 280.0, 22.0), 14.0, M_BOTTOM | M_W);
    let convert = button(p, "Convert", place(h, 24.0, 192.0, 210.0, 34.0), "convert:", M_BOTTOM);
    let status = label(p, "Choose your raw DNA file to begin", place(h, 250.0, 198.0, w - 280.0, 22.0), 14.0, M_BOTTOM | M_W);
    msg![(); status, "setTextColor:", sys_color("secondaryLabelColor") => Id];
    let text = text_area(p, place(h, 28.0, 244.0, w - 56.0, h - 320.0), ui_font(14.0), M_W | M_H);
    let copy = button(p, "Copy coordinates", place(h, 24.0, h - 62.0, 210.0, 34.0), "copyCoords:", M_TOP);
    msg![(); copy, "setEnabled:", false => bool];
    (name, file, status, convert, text, copy)
}

unsafe fn build_heatmap(p: Id, w: f64, h: f64) -> (Id, Id, Id) {
    page_title(p, h, "Heatmap");
    let mode = segmented(p, &["Modern", "Ancient"], place(h, w - 210.0, 28.0, 180.0, 28.0), "mapMode:", M_LEFT | M_BOTTOM);
    let mr = place(h, 28.0, 76.0, w - 56.0, h - 136.0);
    let map = msg![Id; msg![Id; cls("G25MapView"), "alloc"], "initWithFrame:", mr => NSRect];
    add(p, map, M_W | M_H);
    set_layer_bg(map, color(0.863, 0.910, 0.949, 1.0), 10.0);
    msg![(); msg![Id; map, "layer"], "setMasksToBounds:", true => bool];
    let ta = msg![Id; msg![Id; cls("NSTrackingArea"), "alloc"], "initWithRect:options:owner:userInfo:",
        NSRect::default() => NSRect, (0x01usize | 0x02 | 0x40 | 0x200) => usize, map => Id, NIL => Id];
    msg![(); map, "addTrackingArea:", ta => Id];
    // zoom buttons overlaid on the map's top-right corner
    // and the pins on/off button just below them
    for (i, (icon, action)) in [("plus", "zoomIn:"), ("minus", "zoomOut:"), ("mappin.and.ellipse", "toggleDots:")].iter().enumerate() {
        let top = 10.0 + i as f64 * 34.0 + if i == 2 { 8.0 } else { 0.0 };
        let zb = button(map, "", rect(mr.w - 44.0, top, 32.0, 30.0), action, M_LEFT);
        msg![(); zb, "setBordered:", false => bool];
        msg![(); zb, "setImage:", symbol(icon) => Id];
        msg![(); zb, "setImagePosition:", 1usize => usize];
        msg![(); zb, "setContentTintColor:", sys_color("labelColor") => Id];
        set_layer_bg(zb, sys_color("windowBackgroundColor"), 7.0);
        let layer = msg![Id; zb, "layer"];
        msg![(); layer, "setBorderWidth:", 1.0f64 => f64];
        msg![(); layer, "setBorderColor:", msg![Id; sys_color("separatorColor"), "CGColor"] => Id];
        if i == 2 {
            set_dots_button(zb, false);
        }
    }
    let closest = label(p, "", place(h, 30.0, h - 52.0, w - 60.0, 22.0), 14.0, M_W | M_TOP);
    msg![(); closest, "setTextColor:", sys_color("secondaryLabelColor") => Id];
    (map, mode, closest)
}

#[allow(clippy::type_complexity)]
struct Modeling {
    target: Id,
    src_label: Id,
    builtin: Id,
    builtin_g25: Id,
    illus_toggle: Id,
    npop: Id,
    cycles: Id,
    agg: Id,
    adc: Id,
    run: Id,
    bars: Id,
    mode: Id,
    builtin_toggle: Id,
    admix: [Id; 6],
    dist_ctl: [Id; 3],
    grad_ctl: [Id; 3],
    dist_scroll: Id,
    dist_view: Id,
}

/// Distance mode pools, in the order of the era picker.
const DIST_POOLS: [&str; 4] = ["All", "Modern", "Ancient", "Sources"];

unsafe fn build_modeling(p: Id, w: f64, h: f64) -> Modeling {
    page_title(p, h, "G25 modeling");
    let mode = segmented(p, &["Admixture", "Distance"], place(h, w - 420.0, 30.0, 196.0, 26.0), "modelMode:", M_LEFT | M_BOTTOM);
    let vh = label(p, "Vahaduo-compatible", place(h, w - 210.0, 34.0, 180.0, 20.0), 12.0, M_LEFT | M_BOTTOM);
    msg![(); vh, "setTextColor:", sys_color("secondaryLabelColor") => Id];
    msg![(); vh, "setAlignment:", 2usize => usize]; // NSTextAlignmentRight on arm64

    label(p, "Target", place(h, 30.0, 74.0, 200.0, 18.0), 12.0, M_BOTTOM);
    let target = field(p, place(h, 28.0, 94.0, w - 56.0, 26.0), "", mono(12.0), M_W | M_BOTTOM);
    msg![(); target, "setPlaceholderString:", ns("Name,PC1,…,PC25 — filled in after you convert") => Id];

    label(p, "Sources", place(h, 30.0, 134.0, 100.0, 18.0), 12.0, M_BOTTOM);
    let src_label = label(p, "", place(h, w - 380.0, 134.0, 350.0, 18.0), 12.0, M_LEFT | M_BOTTOM);
    msg![(); src_label, "setAlignment:", 2usize => usize];
    msg![(); src_label, "setTextColor:", sys_color("secondaryLabelColor") => Id];
    let half = (w - 56.0 - 10.0) / 2.0;
    let paste = button(p, "Paste  (add your own G25 sources)", place(h, 24.0, 154.0, half + 8.0, 34.0), "pasteSources:", M_BOTTOM);
    msg![(); paste, "setImage:", symbol("doc.on.clipboard") => Id];
    msg![(); paste, "setImagePosition:", 2usize => usize];
    let load = button(p, "Load file  (add your own sources)", place(h, 28.0 + half + 6.0, 154.0, half + 8.0, 34.0), "loadSources:", M_BOTTOM);
    msg![(); load, "setImage:", symbol("doc.badge.plus") => Id];
    msg![(); load, "setImagePosition:", 2usize => usize];
    let builtin_toggle = toggle_checkbox(p, "Show built-in databases", place(h, 28.0, 194.0, 200.0, 18.0), "toggleBuiltin:", M_BOTTOM);
    msg![(); builtin_toggle, "setState:", 1isize => isize];
    let illus_toggle = toggle_checkbox(p, "IllustrativeDNA eras (instead of Modern / Ancient)", place(h, 236.0, 194.0, 360.0, 18.0), "toggleIllus:", M_BOTTOM);
    let builtin_g25 = segmented(p, &data::G25_SETS, place(h, 28.0, 214.0, 260.0, 28.0), "pickBuiltinG25:", M_BOTTOM);
    msg![(); builtin_g25, "setSelectedSegment:", -1isize => isize];
    msg![(); builtin_g25, "setToolTip:", ns(&format!("G25 population averages: Modern ({}) · Ancient ({})", data::count_lines(data::MODERN), data::count_lines(data::ANCIENT))) => Id];
    let builtin = segmented(p, &data::BUILTIN, place(h, 28.0, 214.0, (w - 56.0).min(700.0), 28.0), "pickBuiltin:", M_BOTTOM);
    msg![(); builtin, "setSelectedSegment:", -1isize => isize];
    msg![(); builtin, "setHidden:", true => bool];

    let l_pop = label(p, "Populations (R#P)", place(h, 30.0, 262.0, 150.0, 22.0), 14.0, M_BOTTOM);
    let npop = field(p, place(h, 182.0, 260.0, 48.0, 24.0), "3-5", ui_font(14.0), M_BOTTOM);
    let l_cyc = label(p, "Cycles", place(h, 250.0, 262.0, 60.0, 22.0), 14.0, M_BOTTOM);
    let cycles = field(p, place(h, 310.0, 260.0, 48.0, 24.0), "1", ui_font(14.0), M_BOTTOM);
    let agg = checkbox(p, "Aggregate", place(h, 378.0, 260.0, 110.0, 24.0), true, M_BOTTOM);
    let adc = checkbox(p, "Distance column", place(h, 492.0, 260.0, 150.0, 24.0), false, M_BOTTOM);
    // Distance mode: how many to list and which populations to compare against
    let l_show = label(p, "Show", place(h, 30.0, 262.0, 46.0, 22.0), 14.0, M_BOTTOM);
    let show_n = field(p, place(h, 78.0, 260.0, 52.0, 24.0), "25", ui_font(14.0), M_BOTTOM);
    let era = segmented(p, &DIST_POOLS, place(h, 146.0, 258.0, 262.0, 28.0), "distPool:", M_BOTTOM);
    // gradient steepness: auto (stretched over the rows shown) or set by the slider
    let grad_auto = toggle_checkbox(p, "Auto steepness", place(h, 418.0, 263.0, 112.0, 18.0), "gradAuto:", M_BOTTOM);
    msg![(); grad_auto, "setState:", 1isize => isize];
    msg![(); grad_auto, "setToolTip:", ns("On: green → red runs from your closest to your furthest row shown, so showing fewer rows makes it steeper.\nOff: set the steepness yourself with the slider.") => Id];
    let ctl = *CONTROLLER.get().unwrap() as Id;
    let grad_steep = msg![Id; cls("NSSlider"), "sliderWithValue:minValue:maxValue:target:action:",
        model::steepness_for_span(0.10) => f64, 0.0f64 => f64, 1.0f64 => f64, ctl => Id, sel("gradSteep:") => Sel];
    msg![(); grad_steep, "setFrame:", place(h, 534.0, 262.0, 70.0, 20.0) => NSRect];
    msg![(); grad_steep, "setControlSize:", 1usize => usize]; // small
    msg![(); grad_steep, "setContinuous:", true => bool];
    msg![(); grad_steep, "setEnabled:", false => bool];
    msg![(); grad_steep, "setToolTip:", ns("Gradient steepness: left = gentle, right = steep (colours turn red sooner)") => Id];
    add(p, grad_steep, M_BOTTOM);
    let grad_val = field(p, place(h, 610.0, 261.0, 56.0, 22.0), "0.100", mono(12.0), M_BOTTOM);
    msg![(); grad_val, "setAlignment:", 2usize => usize];
    msg![(); grad_val, "setTarget:", ctl => Id];
    msg![(); grad_val, "setAction:", sel("gradValue:") => Sel];
    msg![(); grad_val, "setToolTip:", ns(&format!("Steepness as a distance: how far past your closest row a match turns full red.\nSmaller = steeper. Type a value ({:.2}–{:.2}) and press Return.", model::SPAN_STEEP, model::SPAN_GENTLE)) => Id];
    for v in [l_show, show_n, era, grad_auto, grad_steep, grad_val] {
        msg![(); v, "setHidden:", true => bool];
    }
    let run = button(p, "Run", place(h, w - 134.0, 256.0, 110.0, 32.0), "runModel:", M_LEFT | M_BOTTOM);
    msg![(); run, "setKeyEquivalent:", ns("\r") => Id];

    let br = place(h, 28.0, 302.0, w - 56.0, h - 362.0);
    let bars = msg![Id; msg![Id; cls("G25BarsView"), "alloc"], "initWithFrame:", br => NSRect];
    add(p, bars, M_W | M_H);
    set_layer_bg(bars, sys_color("textBackgroundColor"), 10.0);
    // the distance list scrolls, since it can be hundreds of rows long
    let dist_scroll = msg![Id; msg![Id; cls("NSScrollView"), "alloc"], "initWithFrame:", br => NSRect];
    msg![(); dist_scroll, "setHasVerticalScroller:", true => bool];
    msg![(); dist_scroll, "setDrawsBackground:", false => bool];
    let dist_view = msg![Id; msg![Id; cls("G25DistView"), "alloc"], "initWithFrame:", rect(0.0, 0.0, br.w, br.h) => NSRect];
    msg![(); dist_view, "setAutoresizingMask:", M_W => usize];
    msg![(); dist_scroll, "setDocumentView:", dist_view => Id];
    add(p, dist_scroll, M_W | M_H);
    set_layer_bg(dist_scroll, sys_color("textBackgroundColor"), 10.0);
    msg![(); dist_scroll, "setHidden:", true => bool];
    button(p, "Copy", place(h, w - 250.0, h - 54.0, 110.0, 30.0), "copyModel:", M_LEFT | M_TOP);
    button(p, "History", place(h, w - 134.0, h - 54.0, 110.0, 30.0), "showHistory:", M_LEFT | M_TOP);
    Modeling {
        target,
        src_label,
        builtin,
        builtin_g25,
        illus_toggle,
        npop,
        cycles,
        agg,
        adc,
        run,
        bars,
        mode,
        builtin_toggle,
        admix: [l_pop, npop, l_cyc, cycles, agg, adc],
        dist_ctl: [l_show, show_n, era],
        grad_ctl: [grad_auto, grad_steep, grad_val],
        dist_scroll,
        dist_view,
    }
}

unsafe fn build_health(p: Id, w: f64, h: f64) -> Id {
    page_title(p, h, "Health info");
    let sub = label(p, "Common, well-studied variants from your raw file, most important first. Odds, not diagnoses.", place(h, 30.0, 62.0, w - 60.0, 22.0), 14.0, M_W | M_BOTTOM);
    msg![(); sub, "setTextColor:", sys_color("secondaryLabelColor") => Id];
    let sr = place(h, 28.0, 94.0, w - 56.0, h - 116.0);
    let scroll = msg![Id; msg![Id; cls("NSScrollView"), "alloc"], "initWithFrame:", sr => NSRect];
    msg![(); scroll, "setHasVerticalScroller:", true => bool];
    msg![(); scroll, "setDrawsBackground:", false => bool];
    let hv = msg![Id; msg![Id; cls("G25HealthView"), "alloc"], "initWithFrame:", rect(0.0, 0.0, sr.w - 16.0, sr.h) => NSRect];
    msg![(); hv, "setAutoresizingMask:", M_W => usize];
    msg![(); scroll, "setDocumentView:", hv => Id];
    add(p, scroll, M_W | M_H);
    hv
}

pub fn run() {
    unsafe {
        let pool = objc_autoreleasePoolPush();
        let app = msg![Id; cls("NSApplication"), "sharedApplication"];
        msg![bool; app, "setActivationPolicy:", 0isize => isize];
        register_fonts();
        let ctl = register_classes();
        CONTROLLER.set(ctl as usize).ok();
        msg![(); app, "setDelegate:", ctl => Id];
        build_menu(app);
        icon::set_dock_icon(app);

        let win = msg![Id; cls("NSWindow"), "alloc"];
        let win = msg![Id; win, "initWithContentRect:styleMask:backing:defer:",
            rect(0.0, 0.0, WIN_W, WIN_H) => NSRect, 7usize => usize, 2usize => usize, false => bool];
        msg![(); win, "setTitle:", ns("genesloppa") => Id];
        let root = msg![Id; win, "contentView"];

        // sidebar (native translucent sidebar material)
        let sidebar = msg![Id; msg![Id; cls("NSVisualEffectView"), "alloc"], "initWithFrame:", rect(0.0, 0.0, SIDEBAR_W, WIN_H) => NSRect];
        msg![(); sidebar, "setMaterial:", 7isize => isize];
        msg![(); sidebar, "setBlendingMode:", 0isize => isize];
        msg![(); sidebar, "setState:", 1isize => isize];
        add(root, sidebar, M_H);
        let side_title = label(sidebar, "genesloppa", rect(18.0, WIN_H - 50.0, SIDEBAR_W - 70.0, 32.0), 22.0, M_BOTTOM);
        let tog = button(sidebar, "", rect(SIDEBAR_W - 46.0, WIN_H - 48.0, 34.0, 28.0), "toggleSidebar:", M_BOTTOM);
        msg![(); tog, "setBordered:", false => bool];
        msg![(); tog, "setImage:", symbol("sidebar.left") => Id];
        msg![(); tog, "setImagePosition:", 1usize => usize];
        msg![(); tog, "setAutoresizingMask:", (M_BOTTOM) => usize];
        let mut nav = [0usize; PAGES];
        for (i, (title, icon)) in NAV.iter().enumerate() {
            let b = button(sidebar, title, rect(10.0, WIN_H - 92.0 - i as f64 * 40.0, SIDEBAR_W - 20.0, 34.0), "nav:", M_BOTTOM);
            msg![(); b, "setBordered:", false => bool];
            msg![(); b, "setImage:", symbol(icon) => Id];
            msg![(); b, "setImagePosition:", 2usize => usize];
            msg![(); b, "setAlignment:", 0usize => usize];
            msg![(); b, "setTag:", (i as isize) => isize];
            msg![(); b, "setFont:", ui_font(14.0) => Id];
            nav[i] = b as usize;
        }
        let side_footer = label(sidebar, "Sample: me", rect(18.0, 14.0, SIDEBAR_W - 30.0, 18.0), 12.0, M_TOP);
        msg![(); side_footer, "setTextColor:", sys_color("tertiaryLabelColor") => Id];
        // keep the collapse button pinned when the sidebar shrinks
        msg![(); tog, "setFrame:", rect(SIDEBAR_W - 46.0, WIN_H - 48.0, 34.0, 28.0) => NSRect];

        // content area + pages
        let (cw, ch) = (WIN_W - SIDEBAR_W, WIN_H);
        let content = plain_view(root, rect(SIDEBAR_W, 0.0, cw, ch), M_W | M_H);
        let pages: [Id; PAGES] = std::array::from_fn(|_| plain_view(content, rect(0.0, 0.0, cw, ch), M_W | M_H));
        let (name, file, status, convert, conv_text, copy) = build_converter(pages[0], cw, ch);
        let (map, mode, closest) = build_heatmap(pages[1], cw, ch);
        let m = build_modeling(pages[2], cw, ch);
        build_qpadm(pages[3], cw, ch);
        let traits_view = build_physical(pages[4], cw, ch);
        let health = build_health(pages[5], cw, ch);

        W.set(Widgets {
            win: win as usize,
            sidebar: sidebar as usize,
            content: content as usize,
            pages: pages.map(|p| p as usize),
            nav,
            side_title: side_title as usize,
            side_footer: side_footer as usize,
            name: name as usize,
            file: file as usize,
            status: status as usize,
            convert: convert as usize,
            conv_text: conv_text as usize,
            copy: copy as usize,
            map: map as usize,
            mode: mode as usize,
            closest: closest as usize,
            target: m.target as usize,
            src_label: m.src_label as usize,
            builtin: m.builtin as usize,
            builtin_g25: m.builtin_g25 as usize,
            illus_toggle: m.illus_toggle as usize,
            npop: m.npop as usize,
            cycles: m.cycles as usize,
            agg: m.agg as usize,
            adc: m.adc as usize,
            run: m.run as usize,
            bars: m.bars as usize,
            model_mode: m.mode as usize,
            builtin_toggle: m.builtin_toggle as usize,
            admix: m.admix.map(|v| v as usize),
            dist_ctl: m.dist_ctl.map(|v| v as usize),
            grad_ctl: m.grad_ctl.map(|v| v as usize),
            dist_scroll: m.dist_scroll as usize,
            dist_view: m.dist_view as usize,
            traits_view: traits_view as usize,
            health: health as usize,
        })
        .ok();

        TOG.set(tog as usize).ok();

        apply_layout();
        refresh_now();

        // background loading: reference pack, map and places
        std::thread::spawn(|| {
            let _ = PACK.get_or_init(|| Pack::open(&resource("reference.g25pack")));
            qp_request_refresh(); // fills the qpAdm population pickers
            let _ = WORLD.get_or_init(|| geo::load_map(&resource("world.g25map")));
            let _ = PLACES.get_or_init(|| geo::load_places_with_extras(&resource("places.tsv"), &resource("extra_places.tsv")));
            request_refresh();
        });
        #[cfg(feature = "shots")]
        shots::start();

        msg![(); win, "center"];
        msg![(); win, "makeKeyAndOrderFront:", NIL => Id];
        msg![(); app, "activateIgnoringOtherApps:", true => bool];
        objc_autoreleasePoolPop(pool);
        msg![(); app, "run"];
    }
}

// ================================================================ physical characteristics
const CARD_H: f64 = 92.0;
const CARD_GAP: f64 = 10.0;

unsafe fn build_physical(p: Id, w: f64, h: f64) -> Id {
    page_title(p, h, "Physical characteristics");
    let sub = label(p, "Appearance traits predicted from your raw DNA file.", place(h, 30.0, 62.0, w - 60.0, 22.0), 14.0, M_W | M_BOTTOM);
    msg![(); sub, "setTextColor:", sys_color("secondaryLabelColor") => Id];
    let sr = place(h, 28.0, 94.0, w - 56.0, h - 170.0);
    let scroll = msg![Id; msg![Id; cls("NSScrollView"), "alloc"], "initWithFrame:", sr => NSRect];
    msg![(); scroll, "setHasVerticalScroller:", true => bool];
    msg![(); scroll, "setDrawsBackground:", false => bool];
    let content_h = 10.0 * (CARD_H + CARD_GAP) + CARD_GAP;
    let tv = msg![Id; msg![Id; cls("G25TraitsView"), "alloc"], "initWithFrame:", rect(0.0, 0.0, sr.w - 16.0, content_h) => NSRect];
    msg![(); tv, "setAutoresizingMask:", M_W => usize];
    msg![(); scroll, "setDocumentView:", tv => Id];
    add(p, scroll, M_W | M_H);
    let gen = button(p, "  Generate estimate of your likeness", place(h, w / 2.0 - 190.0, h - 64.0, 380.0, 40.0), "generateLikeness:", M_LEFT | M_TOP);
    msg![(); gen, "setImage:", symbol("person.crop.circle") => Id];
    msg![(); gen, "setImagePosition:", 2usize => usize];
    msg![(); gen, "setFont:", ui_font(16.0) => Id];
    tv
}

fn shade(c: [u8; 3], f: f64) -> (f64, f64, f64) {
    ((c[0] as f64 / 255.0 * f).min(1.0), (c[1] as f64 / 255.0 * f).min(1.0), (c[2] as f64 / 255.0 * f).min(1.0))
}
unsafe fn fill_rgb(c: CG, col: (f64, f64, f64)) {
    CGContextSetRGBFillColor(c, col.0, col.1, col.2, 1.0);
}
unsafe fn stroke_rgb(c: CG, col: (f64, f64, f64), w: f64) {
    CGContextSetRGBStrokeColor(c, col.0, col.1, col.2, 1.0);
    CGContextSetLineWidth(c, w);
}
unsafe fn line(c: CG, pts: &[(f64, f64)]) {
    CGContextBeginPath(c);
    CGContextMoveToPoint(c, pts[0].0, pts[0].1);
    for p in &pts[1..] {
        CGContextAddLineToPoint(c, p.0, p.1);
    }
    CGContextStrokePath(c);
}
unsafe fn draw_wrapped(s: &str, r: NSRect, font: Id, col: Id) {
    let keys = [ns("NSFont"), ns("NSColor")];
    let vals = [font, col];
    let attrs = msg![Id; cls("NSDictionary"), "dictionaryWithObjects:forKeys:count:", vals.as_ptr() => *const Id, keys.as_ptr() => *const Id, 2usize => usize];
    msg![(); ns(s), "drawInRect:withAttributes:", r => NSRect, attrs => Id];
}

/// A strand of hair from (x, y0) down to y1 in the given texture.
unsafe fn hair_strand(c: CG, x: f64, y0: f64, y1: f64, tex: Texture, amp: f64) {
    CGContextBeginPath(c);
    CGContextMoveToPoint(c, x, y0);
    match tex {
        Texture::Straight => CGContextAddLineToPoint(c, x, y1),
        Texture::Wavy => {
            let n = 4;
            let step = (y1 - y0) / n as f64;
            for i in 0..n {
                let ya = y0 + step * i as f64;
                let side = if i % 2 == 0 { amp } else { -amp };
                CGContextAddQuadCurveToPoint(c, x + side, ya + step / 2.0, x, ya + step);
            }
        }
        Texture::Curly => {
            let n = 5;
            let step = (y1 - y0) / n as f64;
            for i in 0..n {
                let ya = y0 + step * i as f64;
                CGContextAddCurveToPoint(c, x + amp * 1.4, ya - step * 0.2, x + amp * 1.4, ya + step * 1.0, x, ya + step * 0.5);
                CGContextAddCurveToPoint(c, x - amp * 1.4, ya, x - amp * 1.4, ya + step * 1.1, x, ya + step);
            }
        }
    }
    CGContextStrokePath(c);
}

/// Small cartoon icon for one trait, drawn in a 64x64 box.
unsafe fn draw_icon(c: CG, kind: Kind, r: NSRect, t: &Traits) {
    let path = msg![Id; cls("NSBezierPath"), "bezierPathWithRoundedRect:xRadius:yRadius:", r => NSRect, 12.0f64 => f64, 12.0f64 => f64];
    msg![(); sys_color("quaternaryLabelColor"), "setFill"];
    msg![(); path, "fill"];
    let (x0, y0, cx, cy) = (r.x, r.y, r.x + r.w / 2.0, r.y + r.h / 2.0);
    let dark = (0.22, 0.22, 0.24);
    CGContextSetLineCap(c, 1);
    CGContextSetLineJoin(c, 1);
    match kind {
        Kind::Sex => {
            stroke_rgb(c, dark, 3.0);
            CGContextStrokeEllipseInRect(c, rect(cx - 13.0, cy - 6.0, 22.0, 22.0));
            match t.sex {
                Sex::Male => {
                    line(c, &[(cx + 5.5, cy - 2.5), (cx + 16.0, cy - 13.0)]);
                    line(c, &[(cx + 7.0, cy - 13.0), (cx + 16.0, cy - 13.0), (cx + 16.0, cy - 4.0)]);
                }
                Sex::Female => {
                    line(c, &[(cx - 2.0, cy + 16.0), (cx - 2.0, cy + 27.0)]);
                    line(c, &[(cx - 8.0, cy + 22.0), (cx + 4.0, cy + 22.0)]);
                }
                Sex::Unknown => {}
            }
        }
        Kind::Earwax => {
            // cartoon ear, then a honey drop (wet) or grey flakes (dry)
            let dry = t.list.iter().any(|x| x.kind == Kind::Earwax && x.value.starts_with("Dry"));
            CGContextBeginPath(c);
            CGContextMoveToPoint(c, x0 + 22.0, y0 + 48.0);
            CGContextAddCurveToPoint(c, x0 + 6.0, y0 + 30.0, x0 + 14.0, y0 + 6.0, x0 + 34.0, y0 + 8.0);
            CGContextAddCurveToPoint(c, x0 + 50.0, y0 + 10.0, x0 + 52.0, y0 + 30.0, x0 + 40.0, y0 + 40.0);
            CGContextAddCurveToPoint(c, x0 + 34.0, y0 + 46.0, x0 + 34.0, y0 + 56.0, x0 + 26.0, y0 + 56.0);
            CGContextAddQuadCurveToPoint(c, x0 + 20.0, y0 + 56.0, x0 + 22.0, y0 + 48.0);
            fill_rgb(c, shade(t.skin_rgb, 1.0));
            CGContextFillPath(c);
            CGContextBeginPath(c);
            CGContextMoveToPoint(c, x0 + 22.0, y0 + 48.0);
            CGContextAddCurveToPoint(c, x0 + 6.0, y0 + 30.0, x0 + 14.0, y0 + 6.0, x0 + 34.0, y0 + 8.0);
            CGContextAddCurveToPoint(c, x0 + 50.0, y0 + 10.0, x0 + 52.0, y0 + 30.0, x0 + 40.0, y0 + 40.0);
            CGContextAddCurveToPoint(c, x0 + 34.0, y0 + 46.0, x0 + 34.0, y0 + 56.0, x0 + 26.0, y0 + 56.0);
            CGContextAddQuadCurveToPoint(c, x0 + 20.0, y0 + 56.0, x0 + 22.0, y0 + 48.0);
            stroke_rgb(c, dark, 2.5);
            CGContextStrokePath(c);
            CGContextBeginPath(c);
            CGContextMoveToPoint(c, x0 + 26.0, y0 + 36.0);
            CGContextAddQuadCurveToPoint(c, x0 + 22.0, y0 + 18.0, x0 + 36.0, y0 + 18.0);
            CGContextAddQuadCurveToPoint(c, x0 + 42.0, y0 + 24.0, x0 + 34.0, y0 + 32.0);
            stroke_rgb(c, dark, 2.0);
            CGContextStrokePath(c);
            if dry {
                fill_rgb(c, (0.62, 0.60, 0.56));
                for &(dx, dy) in &[(50.0, 34.0), (54.0, 44.0), (46.0, 50.0)] {
                    CGContextFillRect(c, rect(x0 + dx, y0 + dy, 4.5, 3.0));
                }
            } else {
                CGContextBeginPath(c);
                CGContextMoveToPoint(c, x0 + 51.0, y0 + 32.0);
                CGContextAddQuadCurveToPoint(c, x0 + 58.0, y0 + 44.0, x0 + 51.0, y0 + 48.0);
                CGContextAddQuadCurveToPoint(c, x0 + 44.0, y0 + 44.0, x0 + 51.0, y0 + 32.0);
                fill_rgb(c, (0.85, 0.60, 0.18));
                CGContextFillPath(c);
            }
        }
        Kind::Hairline => {
            // head in profile-ish front view; hairline drawn full, receding, or thin crown
            fill_rgb(c, shade(t.skin_rgb, 1.0));
            CGContextFillEllipseInRect(c, rect(cx - 17.0, y0 + 10.0, 34.0, 44.0));
            fill_rgb(c, shade(t.hair_rgb, 1.0));
            match t.balding {
                0 => {
                    CGContextFillEllipseInRect(c, rect(cx - 19.0, y0 + 6.0, 38.0, 22.0));
                    CGContextFillRect(c, rect(cx - 19.0, y0 + 16.0, 6.0, 16.0));
                    CGContextFillRect(c, rect(cx + 13.0, y0 + 16.0, 6.0, 16.0));
                }
                1 => {
                    CGContextFillEllipseInRect(c, rect(cx - 19.0, y0 + 5.0, 38.0, 17.0));
                    CGContextFillRect(c, rect(cx - 19.0, y0 + 14.0, 5.0, 16.0));
                    CGContextFillRect(c, rect(cx + 14.0, y0 + 14.0, 5.0, 16.0));
                }
                _ => {
                    CGContextFillRect(c, rect(cx - 19.0, y0 + 20.0, 5.0, 14.0));
                    CGContextFillRect(c, rect(cx + 14.0, y0 + 20.0, 5.0, 14.0));
                    stroke_rgb(c, shade(t.hair_rgb, 1.0), 1.5);
                    for dx in [-6.0, 0.0, 6.0] {
                        line(c, &[(cx + dx, y0 + 10.0), (cx + dx * 1.3, y0 + 5.0)]);
                    }
                }
            }
            fill_rgb(c, dark);
            CGContextFillEllipseInRect(c, rect(cx - 9.0, y0 + 34.0, 4.0, 4.0));
            CGContextFillEllipseInRect(c, rect(cx + 5.0, y0 + 34.0, 4.0, 4.0));
        }
        Kind::Eyes => {
            CGContextBeginPath(c);
            CGContextMoveToPoint(c, x0 + 7.0, cy);
            CGContextAddQuadCurveToPoint(c, cx, cy - 26.0, x0 + 57.0, cy);
            CGContextAddQuadCurveToPoint(c, cx, cy + 26.0, x0 + 7.0, cy);
            CGContextClosePath(c);
            fill_rgb(c, (1.0, 1.0, 1.0));
            CGContextFillPath(c);
            fill_rgb(c, shade(t.eye_rgb, 1.0));
            CGContextFillEllipseInRect(c, rect(cx - 11.0, cy - 11.0, 22.0, 22.0));
            fill_rgb(c, (0.05, 0.05, 0.05));
            CGContextFillEllipseInRect(c, rect(cx - 5.0, cy - 5.0, 10.0, 10.0));
            fill_rgb(c, (1.0, 1.0, 1.0));
            CGContextFillEllipseInRect(c, rect(cx + 1.0, cy - 6.0, 4.0, 4.0));
            CGContextBeginPath(c);
            CGContextMoveToPoint(c, x0 + 7.0, cy);
            CGContextAddQuadCurveToPoint(c, cx, cy - 26.0, x0 + 57.0, cy);
            CGContextAddQuadCurveToPoint(c, cx, cy + 26.0, x0 + 7.0, cy);
            stroke_rgb(c, dark, 2.0);
            CGContextStrokePath(c);
        }
        Kind::HairColor => {
            stroke_rgb(c, shade(t.hair_rgb, 1.0), 7.0);
            for (i, dx) in [-14.0, 0.0, 14.0].iter().enumerate() {
                hair_strand(c, cx + dx, y0 + 12.0 + i as f64 * 2.0, y0 + 52.0, Texture::Wavy, 3.0);
            }
        }
        Kind::HairTexture => {
            stroke_rgb(c, shade(t.hair_rgb, 1.15), 3.5);
            hair_strand(c, cx, y0 + 9.0, y0 + 55.0, t.texture, 7.0);
        }
        Kind::HairThickness => {
            stroke_rgb(c, shade(t.hair_rgb, 1.15), 2.0 + t.thickness as f64 * 2.5);
            for dx in [-14.0, 0.0, 14.0] {
                line(c, &[(cx + dx, y0 + 12.0), (cx + dx, y0 + 52.0)]);
            }
        }
        Kind::Skin => {
            for (i, (rgbv, _)) in traits::SKIN_TONES.iter().enumerate() {
                let sel = i == t.skin_index;
                let (bx, bw) = (x0 + 6.0 + i as f64 * 6.6, 6.0);
                let (by, bh) = if sel { (y0 + 8.0, 48.0) } else { (y0 + 16.0, 32.0) };
                fill_rgb(c, shade(*rgbv, 1.0));
                CGContextFillRect(c, rect(bx, by, bw, bh));
                if sel {
                    stroke_rgb(c, dark, 1.5);
                    CGContextStrokeRect(c, rect(bx - 0.5, by - 0.5, bw + 1.0, bh + 1.0));
                }
            }
        }
        Kind::Freckles => {
            fill_rgb(c, shade(t.skin_rgb, 1.0));
            CGContextFillEllipseInRect(c, rect(cx - 22.0, cy - 22.0, 44.0, 44.0));
            fill_rgb(c, shade(t.skin_rgb, 0.68));
            let n = [2, 7, 14][t.freckles as usize];
            let spots = [(-10.0, 4.0), (10.0, 4.0), (-14.0, 9.0), (14.0, 9.0), (-6.0, 8.0), (6.0, 8.0), (0.0, 2.0), (-17.0, 2.0), (17.0, 2.0), (-11.0, 13.0), (11.0, 13.0), (-3.0, 12.0), (3.0, 12.0), (-8.0, -1.0)];
            for &(dx, dy) in spots.iter().take(n) {
                CGContextFillEllipseInRect(c, rect(cx + dx - 1.6, cy + dy - 1.6, 3.2, 3.2));
            }
            fill_rgb(c, dark);
            CGContextFillEllipseInRect(c, rect(cx - 10.0, cy - 9.0, 4.0, 4.0));
            CGContextFillEllipseInRect(c, rect(cx + 6.0, cy - 9.0, 4.0, 4.0));
        }
        Kind::Muscle => {
            let endurance = t.list.iter().any(|x| x.kind == Kind::Muscle && x.value.contains("Endurance"));
            let power = t.list.iter().any(|x| x.kind == Kind::Muscle && x.value.contains("Power"));
            if !power {
                stroke_rgb(c, (0.85, 0.3, 0.3), 3.0);
                line(c, &[(x0 + 6.0, cy + 6.0), (x0 + 20.0, cy + 6.0), (x0 + 26.0, cy - 12.0), (x0 + 33.0, cy + 18.0), (x0 + 39.0, cy + 6.0), (x0 + 58.0, cy + 6.0)]);
            }
            if !endurance {
                fill_rgb(c, (0.94, 0.62, 0.15));
                CGContextBeginPath(c);
                for (i, p) in [(cx + 4.0, y0 + 8.0), (cx - 10.0, cy + 2.0), (cx - 1.0, cy + 2.0), (cx - 5.0, y0 + 56.0), (cx + 11.0, cy - 4.0), (cx + 2.0, cy - 4.0)].iter().enumerate() {
                    if i == 0 { CGContextMoveToPoint(c, p.0, p.1) } else { CGContextAddLineToPoint(c, p.0, p.1) }
                }
                CGContextClosePath(c);
                CGContextFillPath(c);
            }
        }
    }
}

extern "C" fn traits_draw(this: Id, _cmd: Sel, _dirty: NSRect) {
    unsafe {
        let c = current_cg();
        let b = view_bounds(this);
        let Some(t) = with_state(|s| s.traits.clone()) else {
            draw_text("Choose your DNA file on the Converter page — your traits appear here right away.", 8.0, 8.0, ui_font(14.0), sys_color("secondaryLabelColor"));
            return;
        };
        for (i, tr) in t.list.iter().enumerate() {
            let y = CARD_GAP + i as f64 * (CARD_H + CARD_GAP);
            draw_panel(rect(4.0, y, b.w - 8.0, CARD_H));
            draw_icon(c, tr.kind, rect(16.0, y + 14.0, 64.0, 64.0), &t);
            draw_text(tr.title, 96.0, y + 12.0, ui_font(13.0), sys_color("secondaryLabelColor"));
            draw_text(&tr.value, 96.0, y + 30.0, ui_font(18.0), sys_color("labelColor"));
            draw_wrapped(&tr.detail, rect(96.0, y + 56.0, b.w - 230.0, 32.0), mono(10.0), sys_color("secondaryLabelColor"));
            // confidence pill
            let (bg, fg) = match tr.confidence {
                traits::Confidence::High => ((0.918, 0.953, 0.871), (0.153, 0.314, 0.039)),
                traits::Confidence::Medium => ((0.980, 0.933, 0.855), (0.388, 0.220, 0.024)),
                traits::Confidence::Low => ((0.93, 0.93, 0.92), (0.35, 0.35, 0.33)),
            };
            let lab = tr.confidence.label();
            let lw = text_width(lab, ui_font(11.0)) + 16.0;
            let pill = msg![Id; cls("NSBezierPath"), "bezierPathWithRoundedRect:xRadius:yRadius:", rect(b.w - lw - 18.0, y + 12.0, lw, 20.0) => NSRect, 10.0f64 => f64, 10.0f64 => f64];
            msg![(); color(bg.0, bg.1, bg.2, 1.0), "setFill"];
            msg![(); pill, "fill"];
            draw_text(lab, b.w - lw - 10.0, y + 14.0, ui_font(11.0), color(fg.0, fg.1, fg.2, 1.0));
        }
    }
}

// ---------------------------------------------------------------- likeness
extern "C" fn on_likeness(_this: Id, _cmd: Sel, _sender: Id) {
    unsafe {
        if with_state(|s| s.traits.is_none()) {
            if let Some(w) = W.get() {
                msg![(); w.traits_view as Id, "setNeedsDisplay:", true => bool];
            }
            return;
        }
        let &(win, view) = LIKENESS.get_or_init(|| {
            let win = msg![Id; cls("NSWindow"), "alloc"];
            let win = msg![Id; win, "initWithContentRect:styleMask:backing:defer:",
                rect(0.0, 0.0, 860.0, 640.0) => NSRect, 7usize => usize, 2usize => usize, false => bool];
            msg![(); win, "setTitle:", ns("Your likeness (estimate)") => Id];
            msg![(); win, "setReleasedWhenClosed:", false => bool];
            let cv = msg![Id; win, "contentView"];
            let v = msg![Id; msg![Id; cls("G25LikenessView"), "alloc"], "initWithFrame:", rect(0.0, 0.0, 860.0, 640.0) => NSRect];
            add(cv, v, M_W | M_H);
            (win as usize, v as usize)
        });
        msg![(); view as Id, "setNeedsDisplay:", true => bool];
        msg![(); win as Id, "center"];
        msg![(); win as Id, "makeKeyAndOrderFront:", NIL => Id];
    }
}

/// Core Graphics backend for the avatar: maps design space into the view.
struct CgPen {
    c: CG,
    s: f64,
    ox: f64,
    oy: f64,
}
impl CgPen {
    unsafe fn trace(&self, path: &[avatar::Seg]) {
        use avatar::Seg::*;
        let (s, ox, oy) = (self.s, self.ox, self.oy);
        let p = |x: f64, y: f64| (ox + x * s, oy + y * s);
        CGContextBeginPath(self.c);
        for seg in path {
            match *seg {
                M(x, y) => { let q = p(x, y); CGContextMoveToPoint(self.c, q.0, q.1) }
                L(x, y) => { let q = p(x, y); CGContextAddLineToPoint(self.c, q.0, q.1) }
                Q(a, b, x, y) => { let (k, q) = (p(a, b), p(x, y)); CGContextAddQuadCurveToPoint(self.c, k.0, k.1, q.0, q.1) }
                C(a, b, c2, d, x, y) => { let (k1, k2, q) = (p(a, b), p(c2, d), p(x, y)); CGContextAddCurveToPoint(self.c, k1.0, k1.1, k2.0, k2.1, q.0, q.1) }
                Z => CGContextClosePath(self.c),
            }
        }
    }
}
impl avatar::Pen for CgPen {
    fn fill(&mut self, path: &[avatar::Seg], c: avatar::Rgb) {
        unsafe {
            self.trace(path);
            CGContextSetRGBFillColor(self.c, c.0, c.1, c.2, 1.0);
            CGContextFillPath(self.c);
        }
    }
    fn stroke(&mut self, path: &[avatar::Seg], c: avatar::Rgb, w: f64) {
        unsafe {
            self.trace(path);
            CGContextSetRGBStrokeColor(self.c, c.0, c.1, c.2, 1.0);
            CGContextSetLineWidth(self.c, w * self.s);
            CGContextStrokePath(self.c);
        }
    }
}

extern "C" fn likeness_draw(this: Id, _cmd: Sel, _dirty: NSRect) {
    unsafe {
        let c = current_cg();
        let b = view_bounds(this);
        let Some(t) = with_state(|s| s.traits.clone()) else { return };
        let name = with_state(|s| s.name.clone());
        CGContextSetLineCap(c, 1);
        CGContextSetLineJoin(c, 1);
        // white card behind the drawing, like the original template
        let area = rect(20.0, 70.0, (b.w - 300.0).max(300.0), b.h - 90.0);
        let card = msg![Id; cls("NSBezierPath"), "bezierPathWithRoundedRect:xRadius:yRadius:", area => NSRect, 14.0f64 => f64, 14.0f64 => f64];
        msg![(); color(0.86, 0.90, 0.94, 1.0), "setFill"]; // cool blue-grey: no skin, hair or eye colour sits near it
        msg![(); card, "fill"];
        let (vx, vy, vw, vh) = avatar::VIEW;
        let sc = ((area.w - 30.0) / vw).min((area.h - 30.0) / vh);
        let mut pen = CgPen { c, s: sc, ox: area.x + (area.w - vw * sc) / 2.0 - vx * sc, oy: area.y + (area.h - vh * sc) / 2.0 - vy * sc };
        avatar::draw(&mut pen, &avatar::Spec::from_traits(&t));

        // caption
        draw_text(&format!("Estimated likeness · {name}"), 24.0, 20.0, ui_font(20.0), sys_color("labelColor"));
        // the traits that make up this drawing, top to bottom (no icons)
        let lx = area.x + area.w + 28.0;
        draw_text("Built from", lx, 78.0, ui_font(13.0), sys_color("secondaryLabelColor"));
        let mut y = 106.0;
        for k in [Kind::Sex, Kind::Skin, Kind::Eyes, Kind::HairColor, Kind::HairTexture, Kind::Freckles] {
            if let Some(tr) = t.list.iter().find(|x| x.kind == k) {
                draw_text(tr.title, lx, y, ui_font(12.0), sys_color("secondaryLabelColor"));
                draw_wrapped(&tr.value, rect(lx, y + 17.0, b.w - lx - 20.0, 40.0), ui_font(15.0), sys_color("labelColor"));
                y += 70.0;
            }
        }
        draw_text("A cartoon built from your trait predictions — not a photo-accurate face.", 24.0, 48.0, ui_font(12.0), sys_color("tertiaryLabelColor"));
    }
}

// ================================================================ health info (cards like Physical characteristics)
const HEADER_H: f64 = 34.0;
const FOOTNOTE: &str = "These are common variants that nudge odds: not diagnoses. Consumer chips miss most rare serious mutations (BRCA, Lynch, cystic fibrosis). Talk to a doctor before acting on anything here.";

/// Section header text shown above the first card of each section.
fn health_header(sec: health::Section) -> &'static str {
    match sec {
        health::Section::Risk => "Health risks · most alarming first",
        health::Section::Medicine => "Medicine response",
        health::Section::Fun => "Fun facts",
    }
}

const ROW_H: f64 = 38.0;
const ROW_GAP: f64 = 6.0;

/// One drawn entry of the health list.
struct HealthSlot {
    /// index into the items
    i: usize,
    /// section header above this entry
    head: Option<&'static str>,
    /// small "Average for you" sub-header above this entry
    sub: bool,
    /// one-line row instead of a full card
    compact: bool,
    y: f64,
}

/// Order and position of everything on the Health page: per section, the notable findings as
/// full cards (ranked), then the average ones as compact rows. Returns the slots and total height.
fn health_layout(items: &[health::Item]) -> (Vec<HealthSlot>, f64) {
    let mut out = vec![];
    let mut y = 0.0;
    for sec in [health::Section::Risk, health::Section::Medicine, health::Section::Fun] {
        let idx: Vec<usize> = (0..items.len()).filter(|&i| items[i].section == sec).collect();
        if idx.is_empty() {
            continue;
        }
        let (full, rows): (Vec<usize>, Vec<usize>) = idx.into_iter().partition(|&i| items[i].level != health::Level::Typical);
        if !out.is_empty() {
            y += 12.0;
        }
        y += HEADER_H;
        let mut head = Some(health_header(sec));
        for i in full {
            out.push(HealthSlot { i, head: head.take(), sub: false, compact: false, y });
            y += CARD_H + CARD_GAP;
        }
        let mut sub = true;
        for i in rows {
            if sub {
                y += 26.0;
            }
            out.push(HealthSlot { i, head: head.take(), sub, compact: true, y });
            sub = false;
            y += ROW_H + ROW_GAP;
        }
    }
    (out, y + 70.0)
}

unsafe fn size_health_view(v: Id) {
    let h = with_state(|s| s.health_items.as_ref().map(|i| health_layout(i).1)).unwrap_or(200.0);
    let sv = msg![Id; v, "enclosingScrollView"];
    let vis = if sv.is_null() { 0.0 } else { msg![NSSize; sv, "contentSize"].h };
    let f = msg![NSRect; v, "frame"];
    msg![(); v, "setFrameSize:", NSSize { w: f.w, h: h.max(vis) } => NSSize];
    msg![(); v, "setNeedsDisplay:", true => bool];
}

extern "C" fn health_draw(this: Id, _cmd: Sel, _dirty: NSRect) {
    unsafe {
        let c = current_cg();
        let b = view_bounds(this);
        let (items, skin) = with_state(|s| (s.health_items.clone(), s.traits.as_ref().map(|t| t.skin_rgb)));
        let Some(items) = items else {
            draw_text("Choose your DNA file on the Converter page — your health cards appear here right away.", 8.0, 8.0, ui_font(14.0), sys_color("secondaryLabelColor"));
            return;
        };
        let skin = skin.unwrap_or([209, 159, 122]);
        let (slots, total) = health_layout(&items);
        let mut rank = 0;
        for sl in &slots {
            let it = &items[sl.i];
            let y = sl.y;
            if let Some(hd) = sl.head {
                let hy = y - HEADER_H + 8.0 - if sl.sub { 26.0 } else { 0.0 };
                draw_text(hd, 8.0, hy, ui_font(15.0), sys_color("labelColor"));
            }
            if sl.sub {
                let t = if it.section == health::Section::Medicine { "Normal response" } else { "Average for you" };
                draw_text(t, 8.0, y - 22.0, ui_font(12.0), sys_color("secondaryLabelColor"));
            }
            if sl.compact {
                // one line: small icon, topic, finding, and the DNA on the right
                draw_panel(rect(4.0, y, b.w - 8.0, ROW_H));
                CGContextSaveGState(c);
                CGContextTranslateCTM(c, 12.0, y + 4.0);
                CGContextScaleCTM(c, 30.0 / 64.0, 30.0 / 64.0);
                draw_health_icon(c, it.icon, rect(0.0, 0.0, 64.0, 64.0), skin);
                CGContextRestoreGState(c);
                let tw = text_width(&it.title, ui_font(13.0));
                draw_text(&it.title, 52.0, y + 10.0, ui_font(13.0), sys_color("labelColor"));
                draw_text(&it.value, 52.0 + tw + 10.0, y + 10.0, ui_font(13.0), sys_color("secondaryLabelColor"));
                let dna = it.dna.split(" · ").take(2).collect::<Vec<_>>().join(" · ");
                let dw = text_width(&dna, mono(10.0));
                draw_text(&dna, b.w - dw - 18.0, y + 12.0, mono(10.0), sys_color("tertiaryLabelColor"));
                continue;
            }
            draw_panel(rect(4.0, y, b.w - 8.0, CARD_H));
            draw_health_icon(c, it.icon, rect(16.0, y + 14.0, 64.0, 64.0), skin);
            let title = if it.section == health::Section::Risk {
                rank += 1;
                format!("{rank}. {}", it.title)
            } else {
                it.title.clone()
            };
            // leave room for the pill on the right
            let pill_w = text_width(it.level.label(), ui_font(11.0)) + 16.0;
            draw_text(&title, 96.0, y + 12.0, ui_font(13.0), sys_color("secondaryLabelColor"));
            draw_wrapped(&it.value, rect(96.0, y + 30.0, b.w - 120.0 - pill_w, 24.0), ui_font(17.0), sys_color("labelColor"));
            draw_wrapped(&format!("DNA: {}", it.dna), rect(96.0, y + 60.0, b.w - 120.0, 28.0), mono(10.0), sys_color("secondaryLabelColor"));
            let (bg, fg) = match it.level {
                health::Level::Higher => ((0.988, 0.886, 0.878), (0.600, 0.110, 0.090)),
                health::Level::Raised => ((0.980, 0.933, 0.855), (0.388, 0.220, 0.024)),
                health::Level::Typical => ((0.93, 0.93, 0.92), (0.35, 0.35, 0.33)),
                health::Level::Lower => ((0.918, 0.953, 0.871), (0.153, 0.314, 0.039)),
                health::Level::Note => ((0.878, 0.925, 0.988), (0.098, 0.290, 0.580)),
                health::Level::Fun => ((0.937, 0.910, 0.988), (0.380, 0.200, 0.600)),
            };
            let pill = msg![Id; cls("NSBezierPath"), "bezierPathWithRoundedRect:xRadius:yRadius:", rect(b.w - pill_w - 18.0, y + 12.0, pill_w, 20.0) => NSRect, 10.0f64 => f64, 10.0f64 => f64];
            msg![(); color(bg.0, bg.1, bg.2, 1.0), "setFill"];
            msg![(); pill, "fill"];
            draw_text(it.level.label(), b.w - pill_w - 10.0, y + 14.0, ui_font(11.0), color(fg.0, fg.1, fg.2, 1.0));
        }
        draw_wrapped(FOOTNOTE, rect(8.0, total - 62.0, b.w - 16.0, 50.0), ui_font(12.0), sys_color("secondaryLabelColor"));
    }
}

/// Cartoon health icon in a 64x64 tile, same style as the physical-trait icons.
unsafe fn draw_health_icon(c: CG, icon: health::Icon, r: NSRect, skin: [u8; 3]) {
    use health::Icon as I;
    let tile = msg![Id; cls("NSBezierPath"), "bezierPathWithRoundedRect:xRadius:yRadius:", r => NSRect, 12.0f64 => f64, 12.0f64 => f64];
    msg![(); sys_color("quaternaryLabelColor"), "setFill"];
    msg![(); tile, "fill"];
    let (x0, y0, cx, cy) = (r.x, r.y, r.x + r.w / 2.0, r.y + r.h / 2.0);
    let dark = (0.22, 0.22, 0.24);
    let red = (0.86, 0.27, 0.30);
    let white = (1.0, 1.0, 1.0);
    CGContextSetLineCap(c, 1);
    CGContextSetLineJoin(c, 1);
    // fill + dark outline for whatever path `build` adds
    let shape = |build: &dyn Fn(), fill: (f64, f64, f64), w: f64| {
        build();
        fill_rgb(c, fill);
        CGContextFillPath(c);
        build();
        stroke_rgb(c, dark, w);
        CGContextStrokePath(c);
    };
    let heart = |sz: f64, ox: f64, oy: f64| {
        let (hx, hy) = (cx + ox, cy + oy);
        CGContextBeginPath(c);
        CGContextMoveToPoint(c, hx, hy + 18.0 * sz);
        CGContextAddCurveToPoint(c, hx - 30.0 * sz, hy - 2.0 * sz, hx - 16.0 * sz, hy - 22.0 * sz, hx, hy - 9.0 * sz);
        CGContextAddCurveToPoint(c, hx + 16.0 * sz, hy - 22.0 * sz, hx + 30.0 * sz, hy - 2.0 * sz, hx, hy + 18.0 * sz);
        CGContextClosePath(c);
    };
    let drop = |dx: f64, dy: f64, s: f64| {
        CGContextBeginPath(c);
        CGContextMoveToPoint(c, cx + dx, cy + dy - 22.0 * s);
        CGContextAddCurveToPoint(c, cx + dx + 4.0 * s, cy + dy - 10.0 * s, cx + dx + 17.0 * s, cy + dy, cx + dx + 15.0 * s, cy + dy + 9.0 * s);
        CGContextAddCurveToPoint(c, cx + dx + 12.0 * s, cy + dy + 23.0 * s, cx + dx - 12.0 * s, cy + dy + 23.0 * s, cx + dx - 15.0 * s, cy + dy + 9.0 * s);
        CGContextAddCurveToPoint(c, cx + dx - 17.0 * s, cy + dy, cx + dx - 4.0 * s, cy + dy - 10.0 * s, cx + dx, cy + dy - 22.0 * s);
        CGContextClosePath(c);
    };
    match icon {
        I::Heart | I::HeartRhythm => {
            shape(&|| heart(1.0, 0.0, 2.0), red, 2.2);
            if icon == I::HeartRhythm {
                stroke_rgb(c, white, 2.4);
                line(c, &[(x0 + 12.0, cy + 2.0), (x0 + 24.0, cy + 2.0), (x0 + 28.0, cy - 8.0), (x0 + 34.0, cy + 12.0), (x0 + 38.0, cy + 2.0), (x0 + 52.0, cy + 2.0)]);
            } else {
                fill_rgb(c, (1.0, 0.78, 0.78));
                CGContextFillEllipseInRect(c, rect(cx - 12.0, cy - 8.0, 6.0, 6.0));
            }
        }
        I::Eye => {
            let lens = || {
                CGContextBeginPath(c);
                CGContextMoveToPoint(c, x0 + 7.0, cy);
                CGContextAddQuadCurveToPoint(c, cx, cy - 26.0, x0 + 57.0, cy);
                CGContextAddQuadCurveToPoint(c, cx, cy + 26.0, x0 + 7.0, cy);
                CGContextClosePath(c);
            };
            shape(&lens, white, 2.0);
            fill_rgb(c, (0.36, 0.55, 0.72));
            CGContextFillEllipseInRect(c, rect(cx - 11.0, cy - 11.0, 22.0, 22.0));
            fill_rgb(c, (0.05, 0.05, 0.05));
            CGContextFillEllipseInRect(c, rect(cx - 5.0, cy - 5.0, 10.0, 10.0));
            // a blurry spot in the centre of vision
            CGContextSetRGBFillColor(c, 0.55, 0.55, 0.55, 0.55);
            CGContextFillEllipseInRect(c, rect(cx - 3.0, cy - 9.0, 13.0, 11.0));
        }
        I::Brain => {
            let brain = || {
                CGContextBeginPath(c);
                CGContextMoveToPoint(c, cx, y0 + 14.0);
                CGContextAddCurveToPoint(c, cx - 14.0, y0 + 6.0, x0 + 4.0, y0 + 18.0, x0 + 10.0, cy + 2.0);
                CGContextAddCurveToPoint(c, x0 + 4.0, cy + 18.0, cx - 12.0, y0 + 58.0, cx, y0 + 50.0);
                CGContextAddCurveToPoint(c, cx + 12.0, y0 + 58.0, x0 + 60.0, cy + 18.0, x0 + 54.0, cy + 2.0);
                CGContextAddCurveToPoint(c, x0 + 60.0, y0 + 18.0, cx + 14.0, y0 + 6.0, cx, y0 + 14.0);
                CGContextClosePath(c);
            };
            shape(&brain, (0.96, 0.70, 0.74), 2.2);
            stroke_rgb(c, dark, 1.6);
            line(c, &[(cx, y0 + 15.0), (cx, y0 + 49.0)]);
            for (sx, sy) in [(-1.0, 0.0), (1.0, 0.0)] {
                CGContextBeginPath(c);
                CGContextMoveToPoint(c, cx + sx * 6.0, cy - 10.0 + sy);
                CGContextAddQuadCurveToPoint(c, cx + sx * 16.0, cy - 6.0, cx + sx * 10.0, cy + 2.0);
                CGContextAddQuadCurveToPoint(c, cx + sx * 4.0, cy + 8.0, cx + sx * 14.0, cy + 12.0);
                CGContextStrokePath(c);
            }
        }
        I::Clot | I::Blood | I::Iron => {
            shape(&|| drop(0.0, 2.0, 1.0), red, 2.2);
            match icon {
                I::Clot => {
                    fill_rgb(c, (0.45, 0.08, 0.10));
                    for &(dx, dy, rr) in &[(-5.0, 6.0, 5.0), (4.0, 10.0, 4.0), (1.0, 2.0, 3.0)] {
                        CGContextFillEllipseInRect(c, rect(cx + dx - rr, cy + dy - rr, rr * 2.0, rr * 2.0));
                    }
                }
                I::Iron => draw_text("Fe", cx - 8.5, cy - 1.0, ui_font(15.0), color(1.0, 1.0, 1.0, 1.0)),
                _ => {
                    // a few cells, one broken (G6PD)
                    fill_rgb(c, (1.0, 0.78, 0.78));
                    CGContextFillEllipseInRect(c, rect(cx - 9.0, cy + 2.0, 8.0, 8.0));
                    CGContextFillEllipseInRect(c, rect(cx + 2.0, cy + 6.0, 6.0, 6.0));
                }
            }
        }
        I::Liver => {
            let liver = || {
                CGContextBeginPath(c);
                CGContextMoveToPoint(c, x0 + 8.0, cy - 6.0);
                CGContextAddCurveToPoint(c, x0 + 12.0, y0 + 12.0, x0 + 48.0, y0 + 10.0, x0 + 57.0, y0 + 20.0);
                CGContextAddCurveToPoint(c, x0 + 60.0, cy, x0 + 44.0, y0 + 40.0, x0 + 30.0, y0 + 48.0);
                CGContextAddCurveToPoint(c, x0 + 20.0, y0 + 52.0, x0 + 6.0, cy + 6.0, x0 + 8.0, cy - 6.0);
                CGContextClosePath(c);
            };
            shape(&liver, (0.62, 0.30, 0.22), 2.2);
            fill_rgb(c, (0.98, 0.85, 0.45)); // fat droplets
            for &(dx, dy) in &[(-10.0, -2.0), (2.0, -6.0), (8.0, 2.0)] {
                CGContextFillEllipseInRect(c, rect(cx + dx - 3.0, cy + dy - 3.0, 6.0, 6.0));
            }
        }
        I::Lungs | I::Cigarette => {
            if icon == I::Cigarette {
                fill_rgb(c, white);
                CGContextFillRect(c, rect(x0 + 8.0, cy + 6.0, 38.0, 9.0));
                fill_rgb(c, (0.85, 0.55, 0.25));
                CGContextFillRect(c, rect(x0 + 8.0, cy + 6.0, 11.0, 9.0));
                fill_rgb(c, (0.95, 0.35, 0.15));
                CGContextFillRect(c, rect(x0 + 46.0, cy + 6.0, 5.0, 9.0));
                stroke_rgb(c, dark, 2.0);
                CGContextStrokeRect(c, rect(x0 + 8.0, cy + 6.0, 43.0, 9.0));
                stroke_rgb(c, (0.6, 0.6, 0.62), 2.0);
                for k in 0..2 {
                    let sx = x0 + 50.0 + k as f64 * 5.0;
                    CGContextBeginPath(c);
                    CGContextMoveToPoint(c, sx, cy + 2.0);
                    CGContextAddQuadCurveToPoint(c, sx - 5.0, cy - 6.0, sx, cy - 12.0);
                    CGContextAddQuadCurveToPoint(c, sx + 5.0, cy - 18.0, sx, cy - 24.0);
                    CGContextStrokePath(c);
                }
            } else {
                for side in [-1.0, 1.0] {
                    let lobe = || {
                        CGContextBeginPath(c);
                        CGContextMoveToPoint(c, cx + side * 4.0, y0 + 20.0);
                        CGContextAddCurveToPoint(c, cx + side * 22.0, y0 + 10.0, cx + side * 26.0, y0 + 50.0, cx + side * 14.0, y0 + 54.0);
                        CGContextAddCurveToPoint(c, cx + side * 4.0, y0 + 56.0, cx + side * 4.0, y0 + 40.0, cx + side * 4.0, y0 + 20.0);
                        CGContextClosePath(c);
                    };
                    shape(&lobe, (0.95, 0.62, 0.66), 2.0);
                }
                stroke_rgb(c, dark, 2.4);
                line(c, &[(cx, y0 + 8.0), (cx, y0 + 22.0)]);
            }
        }
        I::Scale => {
            let body = || {
                CGContextBeginPath(c);
                CGContextAddEllipseInRect(c, rect(x0 + 8.0, y0 + 10.0, 48.0, 46.0));
            };
            shape(&body, (0.90, 0.90, 0.92), 2.2);
            shape(&|| { CGContextBeginPath(c); CGContextAddEllipseInRect(c, rect(cx - 12.0, y0 + 18.0, 24.0, 16.0)); }, white, 1.8);
            stroke_rgb(c, red, 2.0);
            line(c, &[(cx, y0 + 32.0), (cx + 7.0, y0 + 22.0)]);
        }
        I::Sugar => {
            for &(dx, dy) in &[(-14.0, 4.0), (4.0, 4.0), (-5.0, -12.0)] {
                shape(&|| { CGContextBeginPath(c); CGContextAddRect(c, rect(cx + dx - 1.0, cy + dy - 1.0, 18.0, 18.0)); }, white, 2.0);
            }
            fill_rgb(c, red);
            CGContextFillEllipseInRect(c, rect(x0 + 48.0, y0 + 8.0, 9.0, 9.0));
        }
        I::Spine => {
            for k in 0..5 {
                let yy = y0 + 7.0 + k as f64 * 10.5;
                shape(&|| { CGContextBeginPath(c); CGContextAddRect(c, rect(cx - 10.0 + if k % 2 == 0 { 0.0 } else { 1.5 }, yy, 20.0, 8.0)); }, (0.96, 0.93, 0.84), 1.8);
            }
            fill_rgb(c, red);
            CGContextFillEllipseInRect(c, rect(cx + 10.0, y0 + 48.0, 7.0, 7.0));
        }
        I::Skin => {
            let (sr, sg, sb) = shade(skin, 1.0);
            fill_rgb(c, (sr, sg, sb));
            CGContextFillEllipseInRect(c, rect(cx - 22.0, cy - 20.0, 44.0, 40.0));
            CGContextSetRGBFillColor(c, 0.85, 0.35, 0.35, 0.85);
            CGContextFillEllipseInRect(c, rect(cx - 14.0, cy - 8.0, 14.0, 10.0));
            CGContextFillEllipseInRect(c, rect(cx + 3.0, cy + 2.0, 12.0, 9.0));
            fill_rgb(c, (0.97, 0.94, 0.90));
            for &(dx, dy) in &[(-9.0, -5.0), (7.0, 5.0), (-5.0, -3.0)] {
                CGContextFillEllipseInRect(c, rect(cx + dx, cy + dy, 2.5, 2.5));
            }
        }
        I::Foot => {
            let foot = || {
                CGContextBeginPath(c);
                CGContextMoveToPoint(c, x0 + 22.0, y0 + 12.0);
                CGContextAddCurveToPoint(c, x0 + 10.0, y0 + 30.0, x0 + 14.0, y0 + 56.0, x0 + 30.0, y0 + 56.0);
                CGContextAddCurveToPoint(c, x0 + 52.0, y0 + 56.0, x0 + 50.0, y0 + 32.0, x0 + 40.0, y0 + 14.0);
                CGContextAddCurveToPoint(c, x0 + 36.0, y0 + 8.0, x0 + 26.0, y0 + 6.0, x0 + 22.0, y0 + 12.0);
                CGContextClosePath(c);
            };
            shape(&foot, shade(skin, 1.0), 2.2);
            // the big toe, red and sore
            shape(&|| { CGContextBeginPath(c); CGContextAddEllipseInRect(c, rect(x0 + 34.0, y0 + 4.0, 13.0, 13.0)); }, red, 2.0);
        }
        I::Legs => {
            stroke_rgb(c, shade(skin, 0.85), 7.0);
            line(c, &[(cx - 8.0, y0 + 8.0), (cx - 10.0, y0 + 50.0), (cx - 2.0, y0 + 50.0)]);
            line(c, &[(cx + 8.0, y0 + 8.0), (cx + 10.0, y0 + 50.0), (cx + 18.0, y0 + 50.0)]);
            stroke_rgb(c, dark, 1.8);
            for &(sx, sy) in &[(x0 + 7.0, cy - 6.0), (x0 + 7.0, cy + 4.0), (x0 + 53.0, cy - 6.0), (x0 + 53.0, cy + 4.0)] {
                let d = if sx < cx { 1.0 } else { -1.0 };
                CGContextBeginPath(c);
                CGContextMoveToPoint(c, sx, sy);
                CGContextAddQuadCurveToPoint(c, sx + d * 4.0, sy + 3.0, sx, sy + 6.0);
                CGContextStrokePath(c);
            }
        }
        I::Shield => {
            let sh = || {
                CGContextBeginPath(c);
                CGContextMoveToPoint(c, cx, y0 + 8.0);
                CGContextAddLineToPoint(c, x0 + 54.0, y0 + 16.0);
                CGContextAddCurveToPoint(c, x0 + 54.0, y0 + 40.0, cx + 10.0, y0 + 52.0, cx, y0 + 57.0);
                CGContextAddCurveToPoint(c, cx - 10.0, y0 + 52.0, x0 + 10.0, y0 + 40.0, x0 + 10.0, y0 + 16.0);
                CGContextClosePath(c);
            };
            shape(&sh, (0.38, 0.58, 0.86), 2.2);
            // a crack: the immune system turning on the body
            stroke_rgb(c, white, 2.4);
            line(c, &[(cx + 2.0, y0 + 14.0), (cx - 4.0, cy - 2.0), (cx + 4.0, cy + 4.0), (cx - 2.0, y0 + 48.0)]);
        }
        I::Nerve => {
            stroke_rgb(c, dark, 2.0);
            for &(ex, ey) in &[(x0 + 8.0, y0 + 10.0), (x0 + 8.0, y0 + 34.0), (x0 + 26.0, y0 + 6.0)] {
                line(c, &[(x0 + 22.0, y0 + 22.0), (ex, ey)]);
            }
            shape(&|| { CGContextBeginPath(c); CGContextAddEllipseInRect(c, rect(x0 + 13.0, y0 + 13.0, 18.0, 18.0)); }, (0.98, 0.80, 0.45), 2.0);
            // axon with myelin; one sheath missing
            stroke_rgb(c, dark, 2.0);
            line(c, &[(x0 + 28.0, y0 + 28.0), (x0 + 56.0, y0 + 56.0)]);
            for (k, t) in [0.25, 0.5, 0.75].iter().enumerate() {
                let (mx, my) = (x0 + 28.0 + 28.0 * t, y0 + 28.0 + 28.0 * t);
                if k == 1 {
                    continue;
                }
                CGContextSaveGState(c);
                CGContextTranslateCTM(c, mx, my);
                shape(&|| { CGContextBeginPath(c); CGContextAddEllipseInRect(c, rect(-4.5, -4.5, 9.0, 9.0)); }, white, 1.6);
                CGContextRestoreGState(c);
            }
        }
        I::Pill => {
            CGContextSaveGState(c);
            CGContextTranslateCTM(c, cx, cy);
            let half = |left: bool| {
                CGContextBeginPath(c);
                let s = if left { -1.0 } else { 1.0 };
                CGContextMoveToPoint(c, 0.0, -9.0);
                CGContextAddLineToPoint(c, s * 12.0, -9.0);
                CGContextAddCurveToPoint(c, s * 24.0, -9.0, s * 24.0, 9.0, s * 12.0, 9.0);
                CGContextAddLineToPoint(c, 0.0, 9.0);
                CGContextClosePath(c);
            };
            // tilt it a little
            let (sn, cs) = (0.5f64, 0.866f64);
            let _ = (sn, cs);
            shape(&|| half(true), (0.38, 0.58, 0.86), 2.0);
            shape(&|| half(false), white, 2.0);
            CGContextRestoreGState(c);
        }
        I::Coffee => {
            let cup = || {
                CGContextBeginPath(c);
                CGContextMoveToPoint(c, x0 + 12.0, y0 + 26.0);
                CGContextAddLineToPoint(c, x0 + 46.0, y0 + 26.0);
                CGContextAddCurveToPoint(c, x0 + 46.0, y0 + 50.0, x0 + 12.0, y0 + 50.0, x0 + 12.0, y0 + 26.0);
                CGContextClosePath(c);
            };
            shape(&cup, white, 2.2);
            stroke_rgb(c, dark, 2.2);
            CGContextStrokeEllipseInRect(c, rect(x0 + 43.0, y0 + 30.0, 11.0, 10.0));
            fill_rgb(c, (0.42, 0.26, 0.15));
            CGContextFillRect(c, rect(x0 + 14.0, y0 + 27.0, 30.0, 4.0));
            stroke_rgb(c, (0.6, 0.6, 0.62), 2.0);
            for sx in [x0 + 22.0, x0 + 32.0] {
                CGContextBeginPath(c);
                CGContextMoveToPoint(c, sx, y0 + 21.0);
                CGContextAddQuadCurveToPoint(c, sx - 5.0, y0 + 15.0, sx, y0 + 10.0);
                CGContextStrokePath(c);
            }
        }
        I::Drink => {
            let glass = || {
                CGContextBeginPath(c);
                CGContextMoveToPoint(c, x0 + 18.0, y0 + 10.0);
                CGContextAddLineToPoint(c, x0 + 46.0, y0 + 10.0);
                CGContextAddLineToPoint(c, x0 + 42.0, y0 + 54.0);
                CGContextAddLineToPoint(c, x0 + 22.0, y0 + 54.0);
                CGContextClosePath(c);
            };
            shape(&glass, white, 2.0);
            fill_rgb(c, (0.95, 0.70, 0.20));
            CGContextBeginPath(c);
            CGContextMoveToPoint(c, x0 + 20.2, y0 + 22.0);
            CGContextAddLineToPoint(c, x0 + 43.8, y0 + 22.0);
            CGContextAddLineToPoint(c, x0 + 41.0, y0 + 53.0);
            CGContextAddLineToPoint(c, x0 + 23.0, y0 + 53.0);
            CGContextClosePath(c);
            CGContextFillPath(c);
            fill_rgb(c, white);
            CGContextFillEllipseInRect(c, rect(x0 + 27.0, y0 + 32.0, 3.0, 3.0));
            CGContextFillEllipseInRect(c, rect(x0 + 34.0, y0 + 42.0, 3.0, 3.0));
            glass();
            stroke_rgb(c, dark, 2.0);
            CGContextStrokePath(c);
        }
        I::Asparagus => {
            for (k, dx) in [-9.0, 0.0, 9.0].iter().enumerate() {
                let top = y0 + 8.0 + (k % 2) as f64 * 5.0;
                stroke_rgb(c, (0.35, 0.62, 0.25), 6.0);
                line(c, &[(cx + dx, top + 8.0), (cx + dx * 0.6, y0 + 56.0)]);
                shape(&|| { CGContextBeginPath(c); CGContextAddEllipseInRect(c, rect(cx + dx - 4.5, top, 9.0, 13.0)); }, (0.45, 0.30, 0.55), 1.6);
            }
        }
        I::Sun => {
            stroke_rgb(c, (0.95, 0.65, 0.10), 3.0);
            for k in 0..8 {
                let a = k as f64 * std::f64::consts::FRAC_PI_4;
                line(c, &[(cx + a.cos() * 18.0, cy + a.sin() * 18.0), (cx + a.cos() * 26.0, cy + a.sin() * 26.0)]);
            }
            shape(&|| { CGContextBeginPath(c); CGContextAddEllipseInRect(c, rect(cx - 13.0, cy - 13.0, 26.0, 26.0)); }, (0.99, 0.82, 0.25), 2.0);
        }
        I::Milk => {
            let glass = || {
                CGContextBeginPath(c);
                CGContextMoveToPoint(c, x0 + 19.0, y0 + 9.0);
                CGContextAddLineToPoint(c, x0 + 45.0, y0 + 9.0);
                CGContextAddLineToPoint(c, x0 + 42.0, y0 + 55.0);
                CGContextAddLineToPoint(c, x0 + 22.0, y0 + 55.0);
                CGContextClosePath(c);
            };
            shape(&glass, (0.97, 0.97, 0.95), 2.2);
            stroke_rgb(c, (0.75, 0.75, 0.78), 1.5);
            line(c, &[(x0 + 20.5, y0 + 20.0), (x0 + 43.5, y0 + 20.0)]);
        }
        I::Wheat => {
            stroke_rgb(c, (0.70, 0.55, 0.20), 2.4);
            line(c, &[(cx, y0 + 14.0), (cx, y0 + 58.0)]);
            for k in 0..4 {
                let yy = y0 + 12.0 + k as f64 * 9.0;
                for s in [-1.0, 1.0] {
                    CGContextSaveGState(c);
                    shape(&|| { CGContextBeginPath(c); CGContextAddEllipseInRect(c, rect(cx + if s < 0.0 { -12.0 } else { 1.0 }, yy, 11.0, 7.0)); }, (0.95, 0.80, 0.40), 1.6);
                    CGContextRestoreGState(c);
                }
            }
            shape(&|| { CGContextBeginPath(c); CGContextAddEllipseInRect(c, rect(cx - 4.0, y0 + 5.0, 8.0, 10.0)); }, (0.95, 0.80, 0.40), 1.6);
        }
        I::Broccoli => {
            stroke_rgb(c, (0.55, 0.75, 0.35), 7.0);
            line(c, &[(cx, cy), (cx, y0 + 56.0)]);
            for &(dx, dy, rr) in &[(-12.0, -2.0, 11.0), (12.0, -2.0, 11.0), (0.0, -12.0, 12.0), (0.0, 2.0, 10.0)] {
                shape(&|| { CGContextBeginPath(c); CGContextAddEllipseInRect(c, rect(cx + dx - rr, cy + dy - rr, rr * 2.0, rr * 2.0)); }, (0.25, 0.55, 0.25), 2.0);
            }
        }
    }
}

// ================================================================ qpAdm page
/// Everything the qpAdm results view shows; `text` is the same as plain text (Copy / History).
#[derive(Clone, Default)]
struct QpReport {
    title: String,
    /// 0 = info, 1 = warning, 2 = problem (blocks the run)
    notes: Vec<(u8, String)>,
    model: Option<qpadm::ModelResult>,
    verdict: String,
    drops: Vec<qpadm::ModelResult>,
    /// G25 admixture with the same sources: rows and fit distance
    g25: Option<(Vec<(String, f64)>, f64)>,
    rotation: Vec<qpadm::ModelResult>,
    rotation_tried: usize,
    text: String,
}

struct QpWidgets {
    target: usize,
    pick: usize,
    sources: usize,
    right: usize,
    preset: usize,
    view: usize,
    status: usize,
    cancel: usize,
    buttons: [usize; 3],
    adv: [usize; 5],
    own: usize,
    #[cfg(feature = "shots")]
    pie: usize,
}
static QP: OnceLock<QpWidgets> = OnceLock::new();
static QP_CANCEL: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
/// Bumped by Reset, so a run that finishes after a reset is thrown away.
static QP_GEN: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// Picker entry: name, rough era, sample count, e.g. "Czech_CordedWare · Bronze Age · 19".
fn qp_describe(name: &str, n: usize, modern: &std::collections::HashSet<String>) -> String {
    let lower = name.to_lowercase();
    let toks: Vec<&str> = lower.split(['_', '.']).collect();
    let has = |t: &str| toks.iter().any(|x| *x == t);
    let era = if modern.contains(name) {
        "modern"
    } else if has("hg") || lower.contains("mesolithic") || lower.contains("paleolithic") {
        "hunter-gatherer"
    } else if has("n") || has("en") || has("mn") || has("ln") || has("ppnb") || lower.contains("neolithic") {
        "Neolithic"
    } else if has("c") || lower.contains("eneolithic") || lower.contains("chalcolithic") {
        "Copper Age"
    } else if lower.contains("ba") && toks.iter().any(|x| x.ends_with("ba")) || lower.contains("bronze") || lower.contains("bellbeaker") || lower.contains("cordedware") || lower.contains("yamnaya") {
        "Bronze Age"
    } else if has("ia") || lower.contains("iron") || lower.contains("scythian") {
        "Iron Age"
    } else if lower.contains("roman") || lower.contains("antiquity") || lower.contains("byzantine") {
        "Antiquity"
    } else if lower.contains("medieval") || lower.contains("viking") || lower.contains("avar") || lower.contains("saxon") {
        "Medieval"
    } else {
        "ancient"
    };
    format!("{name} · {era} · {n}")
}
fn qp_name(item: &str) -> String {
    item.split(" · ").next().unwrap_or(item).trim().to_string()
}
fn modern_names() -> std::collections::HashSet<String> {
    data::MODERN.lines().filter_map(|l| l.split(',').next()).map(|x| x.trim().to_string()).filter(|x| !x.is_empty()).collect()
}

unsafe fn build_qpadm(p: Id, w: f64, h: f64) {
    page_title(p, h, "qpAdm");
    let sub = label(p, "Formal admixture test on real genotypes, like ADMIXTOOLS. Suggest, Rotate, then Run.", place(h, 30.0, 62.0, w - 60.0, 22.0), 13.0, M_W | M_BOTTOM);
    msg![(); sub, "setTextColor:", sys_color("secondaryLabelColor") => Id];
    let ctl = *CONTROLLER.get().unwrap() as Id;

    // target + population picker
    label(p, "Target", place(h, 30.0, 92.0, 60.0, 20.0), 13.0, M_BOTTOM);
    let target = msg![Id; msg![Id; cls("NSComboBox"), "alloc"], "initWithFrame:", place(h, 90.0, 88.0, 170.0, 26.0) => NSRect];
    msg![(); target, "setCompletes:", true => bool];
    msg![(); target, "setStringValue:", ns(qpadm::MY_DNA) => Id];
    msg![(); target, "setFont:", ui_font(13.0) => Id];
    msg![(); target, "setToolTip:", ns("\"My DNA\" uses the raw DNA file you chose on the Converter page. Or type any population.") => Id];
    add(p, target, M_BOTTOM);
    let pick = msg![Id; msg![Id; cls("NSComboBox"), "alloc"], "initWithFrame:", place(h, 270.0, 88.0, w - 270.0 - 270.0, 26.0) => NSRect];
    msg![(); pick, "setCompletes:", true => bool];
    msg![(); pick, "setNumberOfVisibleItems:", 18isize => isize];
    msg![(); pick, "setPlaceholderString:", ns("Search populations…") => Id];
    msg![(); pick, "setToolTip:", ns("Name · era · number of samples") => Id];
    msg![(); pick, "setFont:", ui_font(12.0) => Id];
    add(p, pick, M_W | M_BOTTOM);
    let add_s = button(p, "+ Source", place(h, w - 262.0, 86.0, 82.0, 30.0), "qpAdd:", M_LEFT | M_BOTTOM);
    msg![(); add_s, "setTag:", 0isize => isize];
    let add_r = button(p, "+ Right", place(h, w - 178.0, 86.0, 76.0, 30.0), "qpAdd:", M_LEFT | M_BOTTOM);
    msg![(); add_r, "setTag:", 1isize => isize];
    let tgt_btn = button(p, "Target", place(h, w - 100.0, 86.0, 72.0, 30.0), "qpAdd:", M_LEFT | M_BOTTOM);
    msg![(); tgt_btn, "setTag:", 2isize => isize];
    for b in [add_s, add_r, tgt_btn] {
        msg![(); b, "setFont:", ui_font(12.0) => Id];
    }

    // sources and right lists
    let half = (w - 56.0 - 12.0) / 2.0;
    label(p, "Sources (Left), one per line", place(h, 30.0, 124.0, 220.0, 18.0), 12.0, M_BOTTOM);
    let suggest = button(p, "Suggest from G25", place(h, 28.0 + half - 140.0, 120.0, 140.0, 24.0), "qpSuggest:", M_BOTTOM);
    msg![(); suggest, "setFont:", ui_font(12.0) => Id];
    msg![(); suggest, "setToolTip:", ns("Fills Sources with the closest, varied ancient populations to your target by G25 distance.") => Id];
    label(p, "Right populations", place(h, 40.0 + half, 124.0, 140.0, 18.0), 12.0, M_BOTTOM);
    let preset = msg![Id; msg![Id; cls("NSPopUpButton"), "alloc"], "initWithFrame:pullsDown:", place(h, 28.0 + 2.0 * half + 12.0 - 230.0, 119.0, 230.0, 26.0) => NSRect, true => bool];
    let titles: Vec<Id> = std::iter::once(ns("Ready-made Right set…")).chain(qpadm::RIGHT_PRESETS.iter().map(|x| ns(&format!("{} — {}", x.0, x.1)))).collect();
    msg![(); preset, "addItemsWithTitles:", ns_array(&titles) => Id];
    msg![(); preset, "setTarget:", ctl => Id];
    msg![(); preset, "setAction:", sel("qpPreset:") => Sel];
    msg![(); preset, "setFont:", ui_font(12.0) => Id];
    add(p, preset, M_BOTTOM);
    let sources = text_area(p, place(h, 28.0, 148.0, half, 96.0), mono(11.0), M_BOTTOM);
    let right = text_area(p, place(h, 40.0 + half, 148.0, half, 96.0), mono(11.0), M_BOTTOM);
    for tv in [sources, right] {
        msg![(); tv, "setEditable:", true => bool];
        msg![(); tv, "setAutomaticQuoteSubstitutionEnabled:", false => bool];
        msg![(); tv, "setAutomaticDashSubstitutionEnabled:", false => bool];
        no_wrap(tv);
    }
    msg![(); right, "setString:", ns(&qpadm::RIGHT_PRESETS[0].2.join("\n")) => Id];

    // actions
    let check = button(p, "Check", place(h, 24.0, 252.0, 90.0, 32.0), "qpCheck:", M_BOTTOM);
    msg![(); check, "setToolTip:", ns("Pre-flight check: warns when the Right set can't tell your sources apart, when there are too many Right populations, or when a population has few samples or SNPs.") => Id];
    let run = button(p, "Run model", place(h, 116.0, 252.0, 110.0, 32.0), "qpRun:", M_BOTTOM);
    msg![(); run, "setKeyEquivalent:", ns("\r") => Id];
    let rotate = button(p, "Rotate", place(h, 228.0, 252.0, 90.0, 32.0), "qpRotate:", M_BOTTOM);
    msg![(); rotate, "setToolTip:", ns("Tries every 1-, 2- and 3-source model from your Sources list and ranks the ones that fit.") => Id];
    // Cancel takes Rotate's place while something runs
    let cancel = button(p, "Cancel", place(h, 228.0, 252.0, 90.0, 32.0), "qpCancel:", M_BOTTOM);
    msg![(); cancel, "setHidden:", true => bool];
    let own = msg![Id; msg![Id; cls("NSPopUpButton"), "alloc"], "initWithFrame:pullsDown:", place(h, 324.0, 254.0, 124.0, 28.0) => NSRect, true => bool];
    msg![(); own, "setTarget:", ctl => Id];
    msg![(); own, "setAction:", sel("qpOwn:") => Sel];
    msg![(); own, "setFont:", ui_font(12.0) => Id];
    msg![(); own, "setToolTip:", ns("Make your own population: load raw DNA files (family, friends), or merge existing populations into one group. Use it as Target, Source or Right.") => Id];
    add(p, own, M_BOTTOM);
    let reset = button(p, "Reset", place(h, w - 92.0, 252.0, 66.0, 32.0), "qpReset:", M_LEFT | M_BOTTOM);
    msg![(); reset, "setToolTip:", ns("Start over: target back to My DNA, no sources, the default Right set, default settings, results cleared.") => Id];
    let adv = toggle_checkbox(p, "Advanced", place(h, 458.0, 259.0, 88.0, 18.0), "qpAdvanced:", M_BOTTOM);
    let all = checkbox(p, "All SNPs", place(h, 546.0, 256.0, 80.0, 24.0), false, M_BOTTOM);
    msg![(); all, "setFont:", ui_font(12.0) => Id];
    msg![(); all, "setToolTip:", ns("allsnps: each pair of populations uses every SNP both have, instead of only SNPs shared by everyone in the model.") => Id];
    let l_blk = label(p, "Block Mb", place(h, 626.0, 259.0, 54.0, 18.0), 12.0, M_BOTTOM);
    let blk = field(p, place(h, 680.0, 256.0, 32.0, 22.0), "5", ui_font(12.0), M_BOTTOM);
    let fudge = field(p, place(h, 716.0, 256.0, 52.0, 22.0), "0.0001", ui_font(12.0), M_BOTTOM);
    msg![(); fudge, "setToolTip:", ns("fudge: small amount added to the covariance diagonal") => Id];
    for v in [all, l_blk, blk, fudge] {
        msg![(); v, "setHidden:", true => bool];
    }
    let copy = button(p, "Copy", place(h, w - 330.0, h - 50.0, 80.0, 30.0), "qpCopy:", M_LEFT | M_TOP);
    let export = button(p, "Export R script", place(h, w - 246.0, h - 50.0, 130.0, 30.0), "qpExport:", M_LEFT | M_TOP);
    msg![(); export, "setToolTip:", ns("Copies an ADMIXTOOLS 2 (R) script that runs the same model, so anyone can check it with the standard tool.") => Id];
    let hist = button(p, "History", place(h, w - 112.0, h - 50.0, 90.0, 30.0), "showHistory:", M_LEFT | M_TOP);
    for b in [copy, export, hist] {
        msg![(); b, "setFont:", ui_font(13.0) => Id];
    }
    let pie = toggle_checkbox(p, "Pie chart", place(h, w - 430.0, h - 45.0, 92.0, 20.0), "qpPie:", M_LEFT | M_TOP);
    msg![(); pie, "setToolTip:", ns("Also draw the result as a pie chart.") => Id];
    let status = label(p, "", place(h, 30.0, h - 44.0, w - 470.0, 20.0), 12.0, M_W | M_TOP);
    msg![(); status, "setTextColor:", sys_color("secondaryLabelColor") => Id];

    // results
    let sr = place(h, 28.0, 292.0, w - 56.0, h - 292.0 - 58.0);
    let scroll = msg![Id; msg![Id; cls("NSScrollView"), "alloc"], "initWithFrame:", sr => NSRect];
    msg![(); scroll, "setHasVerticalScroller:", true => bool];
    msg![(); scroll, "setDrawsBackground:", false => bool];
    let view = msg![Id; msg![Id; cls("G25QpadmView"), "alloc"], "initWithFrame:", rect(0.0, 0.0, sr.w - 16.0, sr.h) => NSRect];
    msg![(); view, "setAutoresizingMask:", M_W => usize];
    msg![(); scroll, "setDocumentView:", view => Id];
    add(p, scroll, M_W | M_H);
    set_layer_bg(scroll, sys_color("textBackgroundColor"), 10.0);

    QP.set(QpWidgets {
        target: target as usize,
        pick: pick as usize,
        sources: sources as usize,
        right: right as usize,
        preset: preset as usize,
        view: view as usize,
        status: status as usize,
        cancel: cancel as usize,
        buttons: [check as usize, run as usize, rotate as usize],
        adv: [all as usize, l_blk as usize, blk as usize, fudge as usize, adv as usize],
        own: own as usize,
        #[cfg(feature = "shots")]
        pie: pie as usize,
    })
    .ok();
    qp_own_menu();
}

static QP_ITEMS: OnceLock<Vec<String>> = OnceLock::new();
/// customs_version() the pickers were last filled with
static QP_PICK_VER: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(usize::MAX);
/// a name to put in the search box at the next refresh (a population just made)
static QP_PICK_NEXT: Mutex<Option<String>> = Mutex::new(None);
static QP_PIE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Fill the pickers once the reference pack has loaded, and again when your own populations change.
fn qp_fill_pickers() {
    use std::sync::atomic::Ordering::Relaxed;
    let Some(Ok(pack)) = PACK.get() else { return };
    let ver = qpadm::customs_version();
    if QP_PICK_VER.load(Relaxed) == ver {
        return;
    }
    QP_PICK_VER.store(ver, Relaxed);
    let items = QP_ITEMS.get_or_init(|| {
        let modern = modern_names();
        qpadm::populations(pack).iter().map(|(n, c)| qp_describe(n, *c, &modern)).collect()
    });
    let own: Vec<String> = qpadm::customs().iter().map(|c| format!("{} · your own · {}", c.name, c.samples)).collect();
    unsafe {
        let Some(q) = QP.get() else { return };
        let ids: Vec<Id> = own.iter().chain(items.iter()).map(|x| ns(x)).collect();
        let arr = ns_array(&ids);
        msg![(); q.pick as Id, "removeAllItems"];
        msg![(); q.pick as Id, "addItemsWithObjectValues:", arr => Id];
        msg![(); q.target as Id, "removeAllItems"];
        msg![(); q.target as Id, "addItemWithObjectValue:", ns(qpadm::MY_DNA) => Id];
        msg![(); q.target as Id, "addItemsWithObjectValues:", arr => Id];
    }
    qp_own_menu();
}

/// The "Your own" pull-down: add, merge, and one Remove item per population you made.
fn qp_own_menu() {
    unsafe {
        let Some(q) = QP.get() else { return };
        let b = q.own as Id;
        msg![(); b, "removeAllItems"];
        let mut titles = vec!["Your own…".to_string(), "Load raw DNA files…".into(), "Merge populations…".into()];
        let own = qpadm::customs();
        for c in &own {
            titles.push(format!("Remove {} ({})", c.name, c.what));
        }
        let ids: Vec<Id> = titles.iter().map(|t| ns(t)).collect();
        msg![(); b, "addItemsWithTitles:", ns_array(&ids) => Id];
        if !own.is_empty() {
            let menu = msg![Id; b, "menu"];
            msg![(); menu, "insertItem:atIndex:", msg![Id; cls("NSMenuItem"), "separatorItem"] => Id, 3isize => isize];
        }
    }
}

/// G25 position of a qpAdm population: My DNA, one of your own, or a reference population.
fn qp_coords(name: &str) -> Option<Vec<f64>> {
    if name == qpadm::MY_DNA {
        return with_state(|s| s.coords.clone());
    }
    if let Some(c) = qpadm::custom(name) {
        return c.g25;
    }
    let Some(Ok(pack)) = PACK.get() else { return None };
    pack.pops.iter().find(|p| p.name == name).map(|p| p.coords.to_vec())
}

/// A modal dialog with a text field; returns the text, or None on Cancel.
unsafe fn ask_text(title: &str, info: &str, default: &str, ok: &str) -> Option<String> {
    let alert = msg![Id; msg![Id; cls("NSAlert"), "alloc"], "init"];
    msg![(); alert, "setMessageText:", ns(title) => Id];
    msg![(); alert, "setInformativeText:", ns(info) => Id];
    msg![Id; alert, "addButtonWithTitle:", ns(ok) => Id];
    msg![Id; alert, "addButtonWithTitle:", ns("Cancel") => Id];
    let f = msg![Id; msg![Id; cls("NSTextField"), "alloc"], "initWithFrame:", rect(0.0, 0.0, 300.0, 24.0) => NSRect];
    msg![(); f, "setStringValue:", ns(default) => Id];
    msg![(); alert, "setAccessoryView:", f => Id];
    msg![(); alert, "layout"];
    msg![(); msg![Id; alert, "window"], "setInitialFirstResponder:", f => Id];
    if msg![isize; alert, "runModal"] != 1000 {
        return None;
    }
    Some(from_ns(msg![Id; f, "stringValue"]))
}

/// The merge dialog: a name and a list of member populations. None on Cancel.
unsafe fn ask_merge() -> Option<(String, Vec<String>)> {
    let alert = msg![Id; msg![Id; cls("NSAlert"), "alloc"], "init"];
    msg![(); alert, "setMessageText:", ns("Merge populations") => Id];
    msg![(); alert, "setInformativeText:", ns("Pools the samples of several populations into one new population, e.g. Balkan = Serbian + Croatian + Bosnian. Put one population per line; the search box's names, your own populations and My DNA all work.") => Id];
    msg![Id; alert, "addButtonWithTitle:", ns("Merge") => Id];
    msg![Id; alert, "addButtonWithTitle:", ns("Cancel") => Id];
    let (vw, vh) = (340.0, 210.0);
    let v = msg![Id; msg![Id; cls("NSView"), "alloc"], "initWithFrame:", rect(0.0, 0.0, vw, vh) => NSRect];
    label(v, "Name", place(vh, 0.0, 4.0, 50.0, 18.0), 12.0, 0);
    let name = field(v, place(vh, 52.0, 0.0, vw - 52.0, 24.0), "", ui_font(13.0), 0);
    msg![(); name, "setPlaceholderString:", ns("e.g. Balkan") => Id];
    label(v, "Populations, one per line", place(vh, 0.0, 34.0, vw, 18.0), 12.0, 0);
    let list = text_area(v, place(vh, 0.0, 56.0, vw, vh - 56.0), mono(11.0), 0);
    msg![(); list, "setEditable:", true => bool];
    msg![(); list, "setAutomaticQuoteSubstitutionEnabled:", false => bool];
    msg![(); list, "setAutomaticDashSubstitutionEnabled:", false => bool];
    no_wrap(list);
    // start from whatever is in the search box
    if let Some(q) = QP.get() {
        let n = qp_name(&from_ns(msg![Id; q.pick as Id, "stringValue"]));
        if !n.is_empty() {
            msg![(); list, "setString:", ns(&format!("{n}\n")) => Id];
        }
    }
    msg![(); alert, "setAccessoryView:", v => Id];
    msg![(); alert, "layout"];
    msg![(); msg![Id; alert, "window"], "setInitialFirstResponder:", name => Id];
    if msg![isize; alert, "runModal"] != 1000 {
        return None;
    }
    Some((from_ns(msg![Id; name, "stringValue"]), lines_of(list)))
}

/// Run a "your own population" job in the background with the page marked busy.
fn qp_own_job(title: &str, job: impl FnOnce(&dyn Fn(String)) -> Result<qpadm::Custom, String> + Send + 'static) {
    let started = with_state(|s| {
        if s.qp_busy {
            return false;
        }
        s.qp_busy = true;
        s.qp_progress = format!("{title}…");
        true
    });
    if !started {
        return;
    }
    qp_request_refresh();
    std::thread::spawn(move || {
        let progress = |t: String| {
            with_state(|s| s.qp_progress = t);
            qp_request_refresh();
        };
        let res = job(&progress);
        with_state(|s| {
            s.qp_busy = false;
            s.qp_progress = match &res {
                Ok(c) => format!("Made {} ({}, {} sample{}). It's in the search box: press + Source, + Right or Target.", c.name, c.what, c.samples, if c.samples == 1 { "" } else { "s" }),
                Err(e) => e.clone(),
            };
        });
        if let Ok(c) = res {
            *QP_PICK_NEXT.lock().unwrap() = Some(format!("{} · your own · {}", c.name, c.samples));
        }
        qp_request_refresh();
    });
}

extern "C" fn on_qp_own(_this: Id, _cmd: Sel, sender: Id) {
    unsafe {
        let i = msg![isize; sender, "indexOfSelectedItem"];
        let own = qpadm::customs();
        let Some(Ok(pack)) = PACK.get() else {
            qp_set_status("The reference pack is still loading…");
            return;
        };
        if i == 1 {
            let panel = msg![Id; cls("NSOpenPanel"), "openPanel"];
            msg![(); panel, "setCanChooseFiles:", true => bool];
            msg![(); panel, "setCanChooseDirectories:", false => bool];
            msg![(); panel, "setAllowsMultipleSelection:", true => bool];
            msg![(); panel, "setMessage:", ns("Choose one or more raw DNA files (23andMe, AncestryDNA, MyHeritage, FTDNA…). Several files become one population.") => Id];
            if msg![isize; panel, "runModal"] != 1 {
                return;
            }
            let urls = msg![Id; panel, "URLs"];
            let paths: Vec<String> = (0..msg![usize; urls, "count"]).map(|k| from_ns(msg![Id; msg![Id; urls, "objectAtIndex:", k => usize], "path"])).collect();
            if paths.is_empty() {
                return;
            }
            let stem = std::path::Path::new(&paths[0]).file_stem().map(|x| x.to_string_lossy().to_string()).unwrap_or_default();
            let default = if paths.len() == 1 { stem.split(['.', ' ']).next().unwrap_or("").to_string() } else { "My_group".into() };
            let info = format!("{} file{} will become one population. Give it a name, e.g. Dad, Cousin or Family.", paths.len(), if paths.len() == 1 { "" } else { "s" });
            let Some(name) = ask_text("Name this population", &info, &default, "Add") else { return };
            if let Err(e) = qpadm::check_name(pack, &name) {
                qp_set_status(&e);
                return;
            }
            qp_own_job("Reading DNA files", move |progress| {
                let mut files = vec![];
                let mut g25: Vec<Vec<f64>> = vec![];
                for (k, p) in paths.iter().enumerate() {
                    progress(format!("Reading file {} of {}…", k + 1, paths.len()));
                    let g = raw::read_raw_file(p).map_err(|e| format!("Can't read {p}: {e}"))?;
                    progress(format!("Finding the G25 position of file {} of {}…", k + 1, paths.len()));
                    if let Ok(r) = engine::convert(pack, &g, &name, &engine::Settings::default(), &|_| {}) {
                        g25.push(r.coords.to_vec());
                    }
                    files.push(g);
                }
                let mean = (g25.len() == files.len()).then(|| (0..g25[0].len()).map(|d| g25.iter().map(|c| c[d]).sum::<f64>() / g25.len() as f64).collect());
                progress("Counting alleles…".into());
                qpadm::add_from_raw(pack, &name, &files, mean)
            });
        } else if i == 2 {
            let Some((name, members)) = ask_merge() else { return };
            if let Err(e) = qpadm::check_name(pack, &name) {
                qp_set_status(&e);
                return;
            }
            qp_own_job("Merging", move |_| {
                // sample-weighted mean G25 position, when every member has one
                let mut sum: Option<Vec<f64>> = None;
                let mut n = 0.0;
                let mut all = true;
                for m in &members {
                    let w = qpadm::custom(m).map(|c| c.samples as f64).or_else(|| qpadm::populations(pack).iter().find(|p| &p.0 == m).map(|p| p.1 as f64)).unwrap_or(1.0);
                    match qp_coords(m) {
                        Some(c) => {
                            let s = sum.get_or_insert(vec![0.0; c.len()]);
                            for (a, b) in s.iter_mut().zip(&c) {
                                *a += b * w;
                            }
                            n += w;
                        }
                        None => all = false,
                    }
                }
                let g25 = sum.filter(|_| all && n > 0.0).map(|s| s.iter().map(|x| x / n).collect());
                let user = RAWG.lock().unwrap().clone();
                qpadm::add_merged(pack, &name, &members, user.as_deref(), g25)
            });
        } else if i >= 4 {
            if let Some(c) = own.get(i as usize - 4) {
                qpadm::remove_custom(&c.name);
                qp_set_status(&format!("Removed {}.", c.name));
            }
        }
    }
}

extern "C" fn on_qp_pie(_this: Id, _cmd: Sel, sender: Id) {
    let on = unsafe { msg![isize; sender, "state"] } == 1;
    QP_PIE.store(on, std::sync::atomic::Ordering::Relaxed);
    on_qp_refresh(NIL, sel("qpRefresh:"), NIL);
}

fn lines_of(tv: Id) -> Vec<String> {
    let t = from_ns(unsafe { msg![Id; tv, "string"] });
    let mut v: Vec<String> = vec![];
    for l in t.lines() {
        let n = qp_name(l);
        if !n.is_empty() && !v.contains(&n) {
            v.push(n);
        }
    }
    v
}

/// Read the page's inputs: target, sources, right, settings.
unsafe fn qp_inputs() -> Option<(String, Vec<String>, Vec<String>, qpadm::Settings)> {
    let q = QP.get()?;
    let target = qp_name(&from_ns(msg![Id; q.target as Id, "stringValue"]));
    let sources = lines_of(q.sources as Id);
    let right = lines_of(q.right as Id);
    let mut st = qpadm::Settings::default();
    st.allsnps = msg![isize; q.adv[0] as Id, "state"] == 1;
    if let Ok(v) = from_ns(msg![Id; q.adv[2] as Id, "stringValue"]).trim().parse::<f64>() {
        st.block_mb = v.clamp(0.5, 50.0);
    }
    if let Ok(v) = from_ns(msg![Id; q.adv[3] as Id, "stringValue"]).trim().parse::<f64>() {
        st.fudge = v.clamp(0.0, 0.1);
    }
    Some((target, sources, right, st))
}

extern "C" fn on_qp_add(_this: Id, _cmd: Sel, sender: Id) {
    unsafe {
        let Some(q) = QP.get() else { return };
        let tag = msg![isize; sender, "tag"];
        let name = qp_name(&from_ns(msg![Id; q.pick as Id, "stringValue"]));
        if name.is_empty() {
            return;
        }
        if tag == 2 {
            msg![(); q.target as Id, "setStringValue:", ns(&name) => Id];
            return;
        }
        let tv = if tag == 0 { q.sources } else { q.right } as Id;
        let mut v = lines_of(tv);
        if !v.contains(&name) {
            v.push(name);
        }
        msg![(); tv, "setString:", ns(&v.join("\n")) => Id];
    }
}

extern "C" fn on_qp_preset(_this: Id, _cmd: Sel, sender: Id) {
    unsafe {
        let i = msg![isize; sender, "indexOfSelectedItem"];
        let Some(q) = QP.get() else { return };
        if i >= 1 {
            let set = qpadm::RIGHT_PRESETS[i as usize - 1].2;
            msg![(); q.right as Id, "setString:", ns(&set.join("\n")) => Id];
        }
        msg![(); q.preset as Id, "selectItemAtIndex:", 0isize => isize];
    }
}

extern "C" fn on_qp_advanced(_this: Id, _cmd: Sel, sender: Id) {
    unsafe {
        let on = msg![isize; sender, "state"] == 1;
        if let Some(q) = QP.get() {
            for v in &q.adv[..4] {
                msg![(); *v as Id, "setHidden:", !on => bool];
            }
        }
    }
}

/// Suggest: the closest ancient populations by G25 distance, skipping near-duplicates.
extern "C" fn on_qp_suggest(_this: Id, _cmd: Sel, _sender: Id) {
    unsafe {
        let Some((target, _, right, _)) = qp_inputs() else { return };
        let Some(Ok(pack)) = PACK.get() else {
            qp_set_status("The reference pack is still loading…");
            return;
        };
        let Some(me) = qp_coords(&target) else {
            qp_set_status(if target == qpadm::MY_DNA {
                "Convert your DNA first (Converter page) so the app knows your G25 position."
            } else {
                "This target has no G25 position, so Suggest can't rank sources for it."
            });
            return;
        };
        let modern = modern_names();
        let mut cand: Vec<(f64, &str, Vec<f64>)> = qpadm::populations(pack)
            .into_iter()
            .filter(|(n, c)| *c >= 2 && !modern.contains(n) && n != &target && !right.contains(n) && !n.contains("_o") && !n.ends_with("_o"))
            .filter_map(|(n, _)| pack.pops.iter().find(|p| p.name == n))
            .map(|p| {
                let d: f64 = p.coords.iter().zip(&me).map(|(a, b)| (a - b) * (a - b)).sum::<f64>().sqrt();
                (d, p.name.as_str(), p.coords.to_vec())
            })
            .collect();
        cand.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap());
        // greedy: close to the target but not near-duplicates of each other
        let mut pick: Vec<(&str, Vec<f64>)> = vec![];
        for (_, n, c) in &cand {
            if pick.len() >= 8 {
                break;
            }
            let far = pick.iter().all(|(_, pc)| pc.iter().zip(c).map(|(a, b)| (a - b) * (a - b)).sum::<f64>().sqrt() > 0.03);
            if far {
                pick.push((n, c.clone()));
            }
        }
        if let Some(q) = QP.get() {
            let v: Vec<&str> = pick.iter().map(|x| x.0).collect();
            msg![(); q.sources as Id, "setString:", ns(&v.join("\n")) => Id];
        }
        qp_set_status(&format!("Suggested {} sources by G25 distance. Press Rotate to test 1–3 source models from them.", pick.len()));
    }
}

fn qp_set_status(t: &str) {
    with_state(|s| s.qp_progress = t.to_string());
    qp_request_refresh();
}
fn qp_request_refresh() {
    if let Some(&c) = CONTROLLER.get() {
        unsafe { msg![(); c as Id, "performSelectorOnMainThread:withObject:waitUntilDone:", sel("qpRefresh:") => Sel, NIL => Id, false => bool] }
    }
}
extern "C" fn on_qp_refresh(_this: Id, _cmd: Sel, _arg: Id) {
    unsafe {
        let Some(q) = QP.get() else { return };
        qp_fill_pickers();
        let vw = msg![NSRect; q.view as Id, "frame"].w;
        if let Some(n) = QP_PICK_NEXT.lock().unwrap().take() {
            msg![(); q.pick as Id, "setStringValue:", ns(&n) => Id];
        }
        let (busy, prog, h) = with_state(|s| (s.qp_busy, s.qp_progress.clone(), s.qp.as_ref().map(|r| qp_height(r, vw))));
        msg![(); q.status as Id, "setStringValue:", ns(&prog) => Id];
        msg![(); q.cancel as Id, "setHidden:", !busy => bool];
        msg![(); q.buttons[2] as Id, "setHidden:", busy => bool];
        msg![(); q.own as Id, "setEnabled:", !busy => bool];
        for b in q.buttons {
            msg![(); b as Id, "setEnabled:", !busy => bool];
        }
        let v = q.view as Id;
        let sv = msg![Id; v, "enclosingScrollView"];
        let vis = if sv.is_null() { 0.0 } else { msg![NSSize; sv, "contentSize"].h };
        let f = msg![NSRect; v, "frame"];
        msg![(); v, "setFrameSize:", NSSize { w: f.w, h: h.unwrap_or(0.0).max(vis) } => NSSize];
        msg![(); v, "setNeedsDisplay:", true => bool];
    }
}

/// Pre-flight check: problems that stop the run, warnings, and a short data summary.
fn qp_preflight(target: &str, sources: &[String], right: &[String], st: &qpadm::Settings)
    -> (Vec<(u8, String)>, Option<(Vec<std::sync::Arc<qpadm::Counts>>, qpadm::F2)>) {
    let mut notes: Vec<(u8, String)> = vec![];
    let Some(Ok(pack)) = PACK.get() else {
        return (vec![(2, "The reference pack is still loading. Try again in a moment.".into())], None);
    };
    if target.is_empty() {
        notes.push((2, "Choose a target.".into()));
    }
    if sources.is_empty() {
        notes.push((2, "Add at least one source (or press Suggest from G25).".into()));
    }
    if right.len() < sources.len() + 1 {
        notes.push((2, format!("{} source(s) need at least {} Right populations; you have {}.", sources.len(), sources.len() + 1, right.len())));
    }
    for s in sources {
        if right.contains(s) {
            notes.push((2, format!("{s} is in both Sources and Right. A population can only be on one side.")));
        }
        if s == target {
            notes.push((2, format!("{s} is the target and a source.")));
        }
    }
    if right.contains(&target.to_string()) {
        notes.push((2, format!("{target} is the target and a Right population.")));
    }
    if right.len() > 12 {
        notes.push((1, format!("{} Right populations. Many Right populations push p-values toward zero (Harney et al. 2021); 6–12 is usually enough.", right.len())));
    }
    if notes.iter().any(|n| n.0 == 2) {
        return (notes, None);
    }
    let user = RAWG.lock().unwrap().clone();
    let names: Vec<String> = std::iter::once(target.to_string()).chain(sources.iter().cloned()).chain(right.iter().cloned()).collect();
    let counts = match qpadm::counts(pack, &names, user.as_deref()) {
        Ok(c) => c,
        Err(e) => return ({ notes.push((2, e)); notes }, None),
    };
    for c in &counts {
        let cov = c.snps_covered();
        if c.samples == 1 && c.name != qpadm::MY_DNA {
            notes.push((1, format!("{} has only 1 sample; results lean on that one person.", c.name)));
        }
        if cov < 20_000 {
            notes.push((1, format!("{} covers only {cov} of the pack's SNPs; standard errors will be large.", c.name)));
        }
    }
    let f2 = qpadm::f2_blocks(pack, &counts, st);
    // Harney 2021: the Right set must relate differently to the sources
    for i in 0..sources.len() {
        for j in i + 1..sources.len() {
            let p = qpadm::same_to_right(&f2, &sources[i], &sources[j], right, st);
            if p > st.p_threshold {
                notes.push((1, format!("The Right set can't tell {} from {} apart (p = {}). Add a Right population closer to one of them, or drop one.", sources[i], sources[j], qpadm::fmt_p(p))));
            }
        }
    }
    if f2.n_snps < 10_000 {
        notes.push((1, format!("Only {} SNPs are shared by every population; try fewer populations or the All SNPs option.", f2.n_snps)));
    }
    notes.push((0, format!("{} SNPs in {} jackknife blocks of {} Mb{}.", f2.n_snps, f2.n_blocks, st.block_mb, if st.allsnps { ", all SNPs per pair" } else { "" })));
    (notes, Some((counts, f2)))
}

/// G25 admixture with the same sources, to compare against.
fn qp_g25(target: &str, sources: &[String]) -> Option<(Vec<(String, f64)>, f64)> {
    // needs the reference pack loaded
    PACK.get()?.as_ref().ok()?;
    let tc: Vec<f64> = qp_coords(target)?;
    let mut sheet = String::new();
    for s in sources {
        let c = qp_coords(s)?;
        sheet.push_str(&format!("{},{}\n", s, c.iter().map(|x| format!("{x:.6}")).collect::<Vec<_>>().join(",")));
    }
    let sh = model::parse_sheet(&sheet).ok()?;
    let f = model::fit(&tc, &sh, &model::Options::default()).ok()?;
    Some((f.rows.clone(), f.distance))
}

fn qp_model_text(r: &qpadm::ModelResult) -> String {
    let mut t = String::new();
    for (i, s) in r.sources.iter().enumerate() {
        t.push_str(&format!("{:>6.1}% ± {:>4.1}  {s}\n", r.weights[i] * 100.0, r.se[i] * 100.0));
    }
    t.push_str(&format!("p = {}  (χ² {:.2}, {} dof)\n", qpadm::fmt_p(r.p), r.chisq, r.dof));
    t
}

fn qp_start(title: &str, job: impl FnOnce() -> QpReport + Send + 'static) {
    let started = with_state(|s| {
        if s.qp_busy {
            return false;
        }
        s.qp_busy = true;
        s.qp_progress = format!("{title}…");
        true
    });
    if !started {
        return;
    }
    QP_CANCEL.store(false, std::sync::atomic::Ordering::Relaxed);
    qp_request_refresh();
    let gen = QP_GEN.load(std::sync::atomic::Ordering::Relaxed);
    std::thread::spawn(move || {
        let t0 = std::time::Instant::now();
        let rep = job();
        let secs = t0.elapsed().as_secs_f64();
        with_state(|s| {
            if gen != QP_GEN.load(std::sync::atomic::Ordering::Relaxed) {
                s.qp_busy = false;
                s.qp_progress = "Reset.".into();
                return;
            }
            if !rep.text.is_empty() {
                s.history.push(rep.text.clone());
            }
            s.qp = Some(rep);
            s.qp_busy = false;
            s.qp_progress = format!("Done in {secs:.1} s (population data cached for next time).");
        });
        qp_request_refresh();
    });
}

extern "C" fn on_qp_check(_this: Id, _cmd: Sel, _sender: Id) {
    let Some((target, sources, right, st)) = (unsafe { qp_inputs() }) else { return };
    qp_start("Checking", move || {
        let (notes, _) = qp_preflight(&target, &sources, &right, &st);
        let ok = !notes.iter().any(|n| n.0 >= 1);
        let mut text = format!("qpAdm pre-flight check · target {target}\n");
        for n in &notes {
            text.push_str(&format!("{} {}\n", ["·", "!", "✗"][n.0 as usize], n.1));
        }
        QpReport {
            title: if ok { "Pre-flight check: all good".into() } else { "Pre-flight check".into() },
            notes,
            text: String::new(),
            verdict: if ok { "Nothing to worry about. Press Run model.".into() } else { String::new() },
            ..Default::default()
        }
    });
}

extern "C" fn on_qp_run(_this: Id, _cmd: Sel, _sender: Id) {
    let Some((target, sources, right, st)) = (unsafe { qp_inputs() }) else { return };
    qp_start("Running qpAdm", move || {
        let (notes, data) = qp_preflight(&target, &sources, &right, &st);
        let mut rep = QpReport { title: format!("qpAdm · {target}"), notes, ..Default::default() };
        let Some((_, f2)) = data else { return rep };
        match qpadm::fit(&f2, &target, &sources, &right, &st) {
            Ok(r) => {
                rep.verdict = qpadm::verdict(&r, &st);
                // drop each source in turn
                if sources.len() >= 2 {
                    for i in 0..sources.len() {
                        let sub: Vec<String> = sources.iter().enumerate().filter(|(j, _)| *j != i).map(|(_, s)| s.clone()).collect();
                        if let Ok(d) = qpadm::fit(&f2, &target, &sub, &right, &st) {
                            rep.drops.push(d);
                        }
                    }
                    if let Some(d) = rep.drops.iter().filter(|d| d.passes(&st)).max_by(|a, b| a.p.partial_cmp(&b.p).unwrap()) {
                        rep.verdict.push_str(&format!(" A simpler model without {} also fits (p = {}).",
                            sources.iter().find(|s| !d.sources.contains(s)).cloned().unwrap_or_default(), qpadm::fmt_p(d.p)));
                    }
                }
                rep.g25 = qp_g25(&target, &sources);
                let mut text = format!("qpAdm · target {target}\nRight: {}\n\n{}\n{}\n", right.join(", "), qp_model_text(&r), rep.verdict);
                if !rep.drops.is_empty() {
                    text.push_str("\nDrop one source:\n");
                    for d in &rep.drops {
                        text.push_str(&format!("  {}: p = {}{}\n", d.sources.join(" + "), qpadm::fmt_p(d.p), if d.passes(&st) { " (fits)" } else { "" }));
                    }
                }
                if let Some((g, dist)) = &rep.g25 {
                    text.push_str(&format!("\nG25 admixture, same sources (distance {:.4}):\n", dist));
                    for (n, f) in g {
                        text.push_str(&format!("  {:>5.1}%  {n}\n", f * 100.0));
                    }
                }
                text.push_str(&format!("\n{} SNPs, {} blocks\n", r.n_snps, r.n_blocks));
                rep.text = text;
                rep.model = Some(r);
            }
            Err(e) => rep.notes.push((2, e)),
        }
        rep
    });
}

/// Rotate: every 1-, 2- and 3-source model from the Sources list, ranked.
extern "C" fn on_qp_rotate(_this: Id, _cmd: Sel, _sender: Id) {
    let Some((target, pool, right, st)) = (unsafe { qp_inputs() }) else { return };
    qp_start("Rotating models", move || {
        let mut rep = QpReport { title: format!("Rotation · {target}"), ..Default::default() };
        if pool.len() > 14 {
            rep.notes.push((2, format!("{} candidates is too many to rotate (max 14). Remove some sources.", pool.len())));
            return rep;
        }
        let (notes, data) = qp_preflight(&target, &pool, &right, &st);
        rep.notes = notes.into_iter().filter(|n| n.0 != 1 || !n.1.starts_with("The Right set can't tell")).collect();
        let Some((_, f2)) = data else { return rep };
        let n = pool.len();
        let mut combos: Vec<Vec<usize>> = vec![];
        for a in 0..n {
            combos.push(vec![a]);
            for b in a + 1..n {
                combos.push(vec![a, b]);
                for c in b + 1..n {
                    combos.push(vec![a, b, c]);
                }
            }
        }
        let combos: Vec<Vec<usize>> = combos.into_iter().filter(|c| right.len() > c.len()).collect();
        let total = combos.len();
        let mut res = vec![];
        for (i, c) in combos.iter().enumerate() {
            if QP_CANCEL.load(std::sync::atomic::Ordering::Relaxed) {
                rep.notes.push((1, format!("Cancelled after {i} of {total} models.")));
                break;
            }
            if i % 5 == 0 {
                with_state(|s| s.qp_progress = format!("Rotating models… {i} of {total}"));
                qp_request_refresh();
            }
            let src: Vec<String> = c.iter().map(|&k| pool[k].clone()).collect();
            if let Ok(r) = qpadm::fit(&f2, &target, &src, &right, &st) {
                res.push(r);
            }
            rep.rotation_tried = i + 1;
        }
        // passing first (fewest sources, then highest p); then the best of the rest
        res.sort_by(|a, b| {
            b.passes(&st).cmp(&a.passes(&st)).then(a.sources.len().cmp(&b.sources.len())).then(b.p.partial_cmp(&a.p).unwrap())
        });
        let passing = res.iter().filter(|r| r.passes(&st)).count();
        rep.verdict = if passing == 0 {
            "No model fits. Try other or more varied sources (Suggest from G25), or another Right set.".into()
        } else {
            format!("{passing} model{} fit{}. The simplest fitting model is listed first.", if passing == 1 { "" } else { "s" }, if passing == 1 { "s" } else { "" })
        };
        res.truncate(passing.max(5).min(25));
        let mut text = format!("qpAdm rotation · target {target}\nRight: {}\n{}\n\n", right.join(", "), rep.verdict);
        for r in &res {
            let w: Vec<String> = r.sources.iter().zip(&r.weights).map(|(s, w)| format!("{:.1}% {s}", w * 100.0)).collect();
            text.push_str(&format!("p = {:<8} {}{}\n", qpadm::fmt_p(r.p), w.join(" + "), if r.passes(&st) { "" } else { "  (fails)" }));
        }
        rep.text = text;
        rep.rotation = res;
        rep
    });
}

extern "C" fn on_qp_cancel(_this: Id, _cmd: Sel, _sender: Id) {
    QP_CANCEL.store(true, std::sync::atomic::Ordering::Relaxed);
}

/// Back to a blank page: My DNA target, no sources, the first Right set, default settings.
/// A rotation still running is cancelled; the population data cache is kept.
extern "C" fn on_qp_reset(_this: Id, _cmd: Sel, _sender: Id) {
    QP_CANCEL.store(true, std::sync::atomic::Ordering::Relaxed);
    QP_GEN.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    unsafe {
        let Some(q) = QP.get() else { return };
        msg![(); q.target as Id, "setStringValue:", ns(qpadm::MY_DNA) => Id];
        msg![(); q.pick as Id, "setStringValue:", ns("") => Id];
        msg![(); q.sources as Id, "setString:", ns("") => Id];
        msg![(); q.right as Id, "setString:", ns(&qpadm::RIGHT_PRESETS[0].2.join("\n")) => Id];
        let st = qpadm::Settings::default();
        msg![(); q.adv[0] as Id, "setState:", 0isize => isize];
        msg![(); q.adv[2] as Id, "setStringValue:", ns(&format!("{}", st.block_mb)) => Id];
        msg![(); q.adv[3] as Id, "setStringValue:", ns(&format!("{}", st.fudge)) => Id];
        msg![(); q.adv[4] as Id, "setState:", 0isize => isize];
        for v in &q.adv[..4] {
            msg![(); *v as Id, "setHidden:", true => bool];
        }
    }
    with_state(|s| {
        s.qp = None;
        s.qp_progress = if s.qp_busy { "Stopping…".into() } else { "Reset.".into() };
    });
    on_qp_refresh(NIL, sel("qpRefresh:"), NIL);
}

fn copy_text(t: &str) {
    unsafe {
        let pb = msg![Id; cls("NSPasteboard"), "generalPasteboard"];
        msg![(); pb, "clearContents"];
        msg![bool; pb, "setString:forType:", ns(t) => Id, ns("public.utf8-plain-text") => Id];
    }
}
extern "C" fn on_qp_copy(_this: Id, _cmd: Sel, _sender: Id) {
    let t = with_state(|s| s.qp.as_ref().map(|r| r.text.clone())).unwrap_or_default();
    if t.is_empty() {
        qp_set_status("Nothing to copy yet. Run a model first.");
    } else {
        copy_text(&t);
        qp_set_status("Result copied.");
    }
}
extern "C" fn on_qp_export(_this: Id, _cmd: Sel, _sender: Id) {
    let Some((target, sources, right, st)) = (unsafe { qp_inputs() }) else { return };
    copy_text(&qpadm::admixtools2_script(&target, &sources, &right, &st));
    let own: Vec<String> = std::iter::once(&target).chain(&sources).chain(&right).filter(|n| qpadm::custom(n).is_some()).cloned().collect();
    if own.is_empty() {
        qp_set_status("ADMIXTOOLS 2 R script copied. Paste it into R or RStudio.");
    } else {
        qp_set_status(&format!("R script copied. {} {} your own, so add those samples to the dataset in R.", own.join(", "), if own.len() == 1 { "is" } else { "are" }));
    }
}

// ---------------------------------------------------------------- qpAdm results view
const QP_COLORS: [(f64, f64, f64); 6] = [(0.20, 0.47, 0.80), (0.89, 0.47, 0.16), (0.30, 0.65, 0.35), (0.74, 0.30, 0.55), (0.55, 0.45, 0.30), (0.40, 0.40, 0.75)];

/// lines a note wraps to at this width (roughly 6.6 px per character at 12.5 pt)
fn qp_note_lines(t: &str, w: f64) -> f64 {
    ((t.chars().count() as f64 * 7.4) / (w - 60.0).max(100.0)).ceil().max(1.0)
}
// ------------------------------------------------------------ pie chart of a model
const PIE_R: f64 = 84.0;
fn pie_height(m: &qpadm::ModelResult) -> f64 {
    let odd = m.weights.iter().any(|w| *w < 0.0 || *w > 1.0);
    36.0 + (2.0 * PIE_R).max(m.sources.len() as f64 * 40.0 + if odd { 40.0 } else { 0.0 }) + 20.0
}
/// Shorten text with "..." until it fits `maxw` points.
unsafe fn fit_text(s: &str, font: Id, maxw: f64) -> String {
    if text_width(s, font) <= maxw {
        return s.to_string();
    }
    let mut v: Vec<char> = s.chars().collect();
    while v.len() > 1 {
        v.pop();
        let t: String = v.iter().collect::<String>() + "…";
        if text_width(&t, font) <= maxw {
            return t;
        }
    }
    "…".into()
}
unsafe fn draw_pie(c: CG, m: &qpadm::ModelResult, w: f64, y0: f64) {
    let lab = sys_color("labelColor");
    let sec = sys_color("secondaryLabelColor");
    draw_text("Pie chart", 16.0, y0 + 8.0, ui_font(13.0), lab);
    let r = PIE_R;
    let (cx, cy) = (24.0 + r, y0 + 36.0 + r);
    // shares below 0% can't be a slice: draw them as 0 and say so
    let shown: Vec<f64> = m.weights.iter().map(|x| x.max(0.0)).collect();
    let total: f64 = shown.iter().sum::<f64>().max(1e-9);
    let mut a = -std::f64::consts::FRAC_PI_2; // start at 12 o'clock, go clockwise
    let mut edges = vec![];
    for (i, sh) in shown.iter().enumerate() {
        let sweep = sh / total * std::f64::consts::TAU;
        if sweep <= 0.0 {
            continue;
        }
        fill_rgb(c, QP_COLORS[i % QP_COLORS.len()]);
        CGContextBeginPath(c);
        CGContextMoveToPoint(c, cx, cy);
        let steps = ((sweep.to_degrees() * 2.0).ceil() as usize).max(2);
        for k in 0..=steps {
            let t = a + sweep * k as f64 / steps as f64;
            CGContextAddLineToPoint(c, cx + r * t.cos(), cy + r * t.sin());
        }
        CGContextClosePath(c);
        CGContextFillPath(c);
        edges.push(a);
        // the share inside big slices
        if sh / total >= 0.08 {
            let mid = a + sweep / 2.0;
            let t = format!("{:.0}%", m.weights[i] * 100.0);
            let f = ui_font(12.5);
            draw_text(&t, cx + 0.62 * r * mid.cos() - text_width(&t, f) / 2.0, cy + 0.62 * r * mid.sin() - 8.0, f, color(1.0, 1.0, 1.0, 1.0));
        }
        a += sweep;
    }
    // thin gaps between slices, in the page's background colour
    if edges.len() > 1 {
        msg![(); sys_color("textBackgroundColor"), "setStroke"];
        CGContextSetLineWidth(c, 2.0);
        for e in edges {
            line(c, &[(cx, cy), (cx + r * e.cos(), cy + r * e.sin())]);
        }
    }
    // legend
    let lx = cx + r + 32.0;
    let mut ly = y0 + 36.0 + (2.0 * r - m.sources.len() as f64 * 40.0).max(0.0) / 2.0;
    let (nf, pf) = (ui_font(13.0), mono(11.5));
    for (i, s) in m.sources.iter().enumerate() {
        fill_rgb(c, QP_COLORS[i % QP_COLORS.len()]);
        CGContextFillRect(c, rect(lx, ly + 3.0, 12.0, 12.0));
        draw_text(&fit_text(s, nf, w - lx - 40.0), lx + 20.0, ly, nf, lab);
        draw_text(&format!("{:.1}% ±{:.1}", m.weights[i] * 100.0, m.se[i] * 100.0), lx + 20.0, ly + 18.0, pf, sec);
        ly += 40.0;
    }
    if m.weights.iter().any(|x| *x < 0.0 || *x > 1.0) {
        draw_wrapped("A share is below 0% or above 100%, so this mix isn't possible; the pie shows the negative share as 0%.",
            rect(lx, ly + 2.0, w - lx - 20.0, 36.0), ui_font(11.5), sec);
    }
}

fn qp_height(r: &QpReport, w: f64) -> f64 {
    let mut h = 44.0 + r.notes.iter().map(|n| qp_note_lines(&n.1, w) * 18.0 + 4.0).sum::<f64>();
    if !r.verdict.is_empty() {
        h += 48.0;
    }
    if let Some(m) = &r.model {
        h += 30.0 + m.sources.len() as f64 * 46.0;
        if QP_PIE.load(std::sync::atomic::Ordering::Relaxed) {
            h += pie_height(m);
        }
    }
    if !r.drops.is_empty() {
        h += 34.0 + r.drops.len() as f64 * 22.0;
    }
    if let Some((g, _)) = &r.g25 {
        h += 34.0 + g.len() as f64 * 22.0;
    }
    if !r.rotation.is_empty() {
        h += 34.0 + r.rotation.len() as f64 * 40.0;
    }
    h + 20.0
}

extern "C" fn qp_draw(this: Id, _cmd: Sel, _dirty: NSRect) {
    unsafe {
        let c = current_cg();
        let b = view_bounds(this);
        let Some(r) = with_state(|s| s.qp.clone()) else {
            draw_wrapped("Start with your target (My DNA uses your raw file), press Suggest from G25 to fill Sources, pick a Right set, then Rotate to find the models that fit, or Run model to test one.\n\nCheck runs a quick pre-flight check first.",
                rect(16.0, 16.0, b.w - 32.0, 120.0), ui_font(14.0), sys_color("secondaryLabelColor"));
            return;
        };
        let lab = sys_color("labelColor");
        let sec = sys_color("secondaryLabelColor");
        let mut y = 14.0;
        draw_text(&r.title, 16.0, y, ui_font(16.0), lab);
        y += 30.0;
        for (lvl, t) in &r.notes {
            let (sym, col) = match lvl {
                2 => ("✗", color(0.80, 0.18, 0.15, 1.0)),
                1 => ("!", color(0.85, 0.55, 0.05, 1.0)),
                _ => ("·", sec),
            };
            draw_text(sym, 18.0, y, ui_font(13.0), col);
            let lines = qp_note_lines(t, b.w);
            draw_wrapped(t, rect(34.0, y, b.w - 50.0, lines * 18.0 + 2.0), ui_font(12.5), if *lvl == 0 { sec } else { lab });
            y += lines * 18.0 + 4.0;
        }
        if !r.verdict.is_empty() {
            let passed = r.model.as_ref().map(|m| m.feasible && m.p >= 0.05).unwrap_or(r.rotation.iter().any(|m| m.feasible && m.p >= 0.05) || r.title.contains("all good"));
            let (bg, fg) = if passed { ((0.918, 0.953, 0.871), (0.153, 0.314, 0.039)) } else { ((0.988, 0.886, 0.878), (0.600, 0.110, 0.090)) };
            let path = msg![Id; cls("NSBezierPath"), "bezierPathWithRoundedRect:xRadius:yRadius:", rect(12.0, y + 2.0, b.w - 24.0, 40.0) => NSRect, 8.0f64 => f64, 8.0f64 => f64];
            msg![(); color(bg.0, bg.1, bg.2, 1.0), "setFill"];
            msg![(); path, "fill"];
            draw_wrapped(&r.verdict, rect(22.0, y + 6.0, b.w - 44.0, 36.0), ui_font(13.0), color(fg.0, fg.1, fg.2, 1.0));
            y += 48.0;
        }
        // weight bars with +/- 1 SE whiskers
        if let Some(m) = &r.model {
            let badge = format!("p = {}   χ² {:.1}, {} dof   {} SNPs", qpadm::fmt_p(m.p), m.chisq, m.dof, m.n_snps);
            draw_text(&badge, 16.0, y + 4.0, mono(11.0), sec);
            y += 30.0;
            let (bx, bw) = (16.0, b.w - 16.0 - 110.0);
            for (i, s) in m.sources.iter().enumerate() {
                let (wgt, se) = (m.weights[i], m.se[i]);
                // name on its own line, bar underneath (names can be long)
                draw_text(s, 16.0, y - 2.0, ui_font(13.0), lab);
                y += 16.0;
                CGContextSetRGBFillColor(c, 0.5, 0.5, 0.5, 0.15);
                CGContextFillRect(c, rect(bx, y + 4.0, bw, 20.0));
                let col = QP_COLORS[i % QP_COLORS.len()];
                fill_rgb(c, col);
                CGContextFillRect(c, rect(bx, y + 4.0, bw * wgt.clamp(0.0, 1.0), 20.0));
                stroke_rgb(c, (0.15, 0.15, 0.17), 1.5);
                let (lo, hi) = ((wgt - se).clamp(0.0, 1.0), (wgt + se).clamp(0.0, 1.0));
                line(c, &[(bx + bw * lo, y + 14.0), (bx + bw * hi, y + 14.0)]);
                line(c, &[(bx + bw * lo, y + 9.0), (bx + bw * lo, y + 19.0)]);
                line(c, &[(bx + bw * hi, y + 9.0), (bx + bw * hi, y + 19.0)]);
                draw_text(&format!("{:.1}% ±{:.1}", wgt * 100.0, se * 100.0), bx + bw + 8.0, y + 6.0, mono(11.0), lab);
                y += 30.0;
            }
            if QP_PIE.load(std::sync::atomic::Ordering::Relaxed) {
                draw_pie(c, m, b.w, y);
                y += pie_height(m);
            }
        }
        // drop one source
        if !r.drops.is_empty() {
            draw_text("Drop one source", 16.0, y + 8.0, ui_font(13.0), lab);
            y += 34.0;
            for d in &r.drops {
                let ok = d.passes(&qpadm::Settings::default());
                draw_text(&format!("{}   p = {}{}", d.sources.join(" + "), qpadm::fmt_p(d.p), if ok { "   fits" } else { "" }), 24.0, y, ui_font(12.5), if ok { lab } else { sec });
                y += 22.0;
            }
        }
        // G25 comparison
        if let Some((g, dist)) = &r.g25 {
            draw_text(&format!("G25 admixture with the same sources (distance {:.4})", dist), 16.0, y + 8.0, ui_font(13.0), lab);
            y += 34.0;
            for (n, f) in g {
                draw_text(&format!("{:>5.1}%  {n}", f * 100.0), 24.0, y, mono(11.5), sec);
                y += 22.0;
            }
        }
        // rotation table
        if !r.rotation.is_empty() {
            draw_text(&format!("Models tried: {}", r.rotation_tried), 16.0, y + 8.0, ui_font(13.0), lab);
            y += 34.0;
            let st = qpadm::Settings::default();
            for m in &r.rotation {
                let ok = m.passes(&st);
                let pill_col = if ok { (0.30, 0.62, 0.30) } else { (0.75, 0.30, 0.28) };
                fill_rgb(c, pill_col);
                CGContextFillEllipseInRect(c, rect(18.0, y + 5.0, 9.0, 9.0));
                draw_text(&format!("p {:<8}", qpadm::fmt_p(m.p)), 34.0, y, mono(11.5), sec);
                // stacked mini bar of the weights
                let (bx, bw) = (140.0, 160.0);
                let mut x = bx;
                for (i, wgt) in m.weights.iter().enumerate() {
                    let col = QP_COLORS[i % QP_COLORS.len()];
                    let ww = bw * wgt.clamp(0.0, 1.0);
                    fill_rgb(c, col);
                    CGContextFillRect(c, rect(x, y + 3.0, ww, 13.0));
                    x += ww;
                }
                let w: Vec<String> = m.sources.iter().zip(&m.weights).map(|(s, w)| format!("{:.0}% {s}", w * 100.0)).collect();
                draw_wrapped(&w.join(" + "), rect(bx + bw + 12.0, y, b.w - bx - bw - 20.0, 36.0), ui_font(12.5), if ok { lab } else { sec });
                y += 40.0;
            }
        }
    }
}

#[cfg(feature = "shots")]
mod shots;

mod icon;
pub use icon::write_icns;
