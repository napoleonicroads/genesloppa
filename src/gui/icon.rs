//! The app icon, drawn from src/logo.rs: `.icns` for the bundle, and the Dock icon when the
//! app runs outside a bundle (`cargo run`).

use super::*;
use crate::logo;

extern "C" {
    fn CGContextClearRect(c: CG, r: NSRect);
}

/// Draws the logo with Core Graphics. At small sizes ink lines are kept at least
/// `min_ink` wide so the outlines don't fade away.
struct IconPen {
    cg: CgPen,
    min_ink: f64,
}
impl avatar::Pen for IconPen {
    fn fill(&mut self, path: &[avatar::Seg], c: avatar::Rgb) {
        self.cg.fill(path, c);
    }
    fn stroke(&mut self, path: &[avatar::Seg], c: avatar::Rgb, w: f64) {
        let w = if c == logo::INK { w.max(self.min_ink) } else { w };
        self.cg.stroke(path, c, w);
    }
}

/// The icon as a `px` x `px` CGImage (caller releases it).
unsafe fn render(px: usize) -> CG {
    let cs = CGColorSpaceCreateDeviceRGB();
    let ctx = CGBitmapContextCreate(std::ptr::null_mut(), px, px, 8, px * 4, cs, 1); // RGBA, premultiplied
    CGColorSpaceRelease(cs);
    CGContextClearRect(ctx, rect(0.0, 0.0, px as f64, px as f64)); // transparent margin
    let k = px as f64 / 1024.0;
    // logo space is y-down; bitmap contexts are y-up
    CGContextTranslateCTM(ctx, 0.0, px as f64);
    CGContextScaleCTM(ctx, k, -k);
    CGContextSetLineCap(ctx, 1);
    CGContextSetLineJoin(ctx, 1);
    let mut pen = IconPen { cg: CgPen { c: ctx, s: 1.0, ox: 0.0, oy: 0.0 }, min_ink: 0.9 / k };
    logo::icon(&mut pen);
    let img = CGBitmapContextCreateImage(ctx);
    CGContextRelease(ctx);
    img
}

unsafe fn png(px: usize) -> Vec<u8> {
    let img = render(px);
    let rep = msg![Id; msg![Id; cls("NSBitmapImageRep"), "alloc"], "initWithCGImage:", img => CG];
    let data = msg![Id; rep, "representationUsingType:properties:", 4usize => usize, msg![Id; cls("NSDictionary"), "dictionary"] => Id];
    let bytes = std::slice::from_raw_parts(msg![*const u8; data, "bytes"], msg![usize; data, "length"]).to_vec();
    msg![(); rep, "release"];
    CGImageRelease(img);
    bytes
}

/// Writes the icon as an .icns file. Only the PNG slots (32 to 1024 px); the 16/32 px
/// 1x slots use an old raw format, and macOS scales the @2x images down for those.
pub fn write_icns(path: &str) -> Result<(), String> {
    const SIZES: [(&[u8; 4], usize); 8] = [
        (b"ic11", 32), (b"ic12", 64), (b"ic07", 128), (b"ic13", 256),
        (b"ic08", 256), (b"ic14", 512), (b"ic09", 512), (b"ic10", 1024),
    ];
    let mut body = vec![];
    unsafe {
        let pool = objc_autoreleasePoolPush();
        for (kind, px) in SIZES {
            let data = png(px);
            body.extend_from_slice(kind);
            body.extend_from_slice(&(data.len() as u32 + 8).to_be_bytes());
            body.extend_from_slice(&data);
        }
        objc_autoreleasePoolPop(pool);
    }
    let mut out = b"icns".to_vec();
    out.extend_from_slice(&(body.len() as u32 + 8).to_be_bytes());
    out.extend_from_slice(&body);
    std::fs::write(path, out).map_err(|e| format!("{path}: {e}"))
}

/// Gives the Dock the logo when the bundle doesn't carry an icon of its own.
pub(super) unsafe fn set_dock_icon(app: Id) {
    let bundle = msg![Id; cls("NSBundle"), "mainBundle"];
    if !msg![Id; bundle, "objectForInfoDictionaryKey:", ns("CFBundleIconFile") => Id].is_null() {
        return;
    }
    let img = render(512);
    let image = msg![Id; msg![Id; cls("NSImage"), "alloc"], "initWithCGImage:size:", img => CG, NSSize { w: 512.0, h: 512.0 } => NSSize];
    msg![(); app, "setApplicationIconImage:", image => Id];
    CGImageRelease(img);
}
