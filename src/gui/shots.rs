//! Renders pages to PNG files without clicking through the app, for screenshots and for
//! checking layout changes. Only built with `cargo run --features shots`.
//!
//!   G25_MAPSHOT=<dir> G25_TARGET=<file with one G25 line>   heatmap, modern/ancient x default/zoomed
//!   G25_HEALTHSHOT=<png>                                    Health page with made-up genotypes
//!   G25_QPSHOT=<png> [G25_QPTARGET=pop or Name=A+B+C] [G25_QPSOURCES=a,b,c] [G25_QPPIE=1]
//!                                                           qpAdm page: Suggest, Rotate, then Run

use super::*;

extern "C" {
    fn CGWindowListCreateImage(bounds: NSRect, list: u32, window: u32, opts: u32) -> CG;
}

pub(super) fn methods() -> [(&'static str, *const c_void); 3] {
    [("mapShot:", on_mapshot as *const c_void), ("healthShot:", on_healthshot as *const c_void), ("qpShot:", on_qpshot as *const c_void)]
}

pub(super) fn start() {
    mapshot_start();
    healthshot_start();
    qpshot_start();
}

/// Heatmap: modern and ancient, default view and zoomed into Europe, one PNG each, then quit.
fn mapshot_start() {
    let (Ok(_), Ok(tf)) = (std::env::var("G25_MAPSHOT"), std::env::var("G25_TARGET")) else { return };
    let Ok(t) = std::fs::read_to_string(&tf) else { return };
    let Ok(sh) = model::parse_sheet(&t) else { return };
    adopt_target(&sh.rows[0]);
    std::thread::spawn(|| loop {
        std::thread::sleep(std::time::Duration::from_millis(300));
        if WORLD.get().is_some() && with_state(|s| s.heat[0].is_some() && s.heat[1].is_some()) {
            if let Some(&c) = CONTROLLER.get() {
                unsafe { msg![(); c as Id, "performSelectorOnMainThread:withObject:waitUntilDone:", sel("mapShot:") => Sel, NIL => Id, false => bool] }
            }
            return;
        }
    });
}
extern "C" fn on_mapshot(_this: Id, _cmd: Sel, _arg: Id) {
    let dir = std::env::var("G25_MAPSHOT").unwrap_or_default();
    unsafe {
        let Some(w) = W.get() else { return };
        let view = w.map as Id;
        let b = view_bounds(view);
        for mode in [0usize, 1] {
            for (zname, z) in [("default", 1.0), ("zoomed", 3.5)] {
                with_state(|s| {
                    s.map_mode = mode;
                    s.hover = None;
                    fit_view(b, &mut s.view);
                });
                if z > 1.0 {
                    // zoom in on central Europe
                    let v = with_state(|s| s.view);
                    zoom_at(NSPoint { x: geo::merc_x(14.0) * v.k + v.tx, y: geo::merc_y(48.0) * v.k + v.ty }, z);
                }
                let m = ["modern", "ancient"][mode];
                render_map_png(view, b, &format!("{dir}/{m}_{zname}.png"));
            }
        }
        msg![(); msg![Id; cls("NSApplication"), "sharedApplication"], "terminate:", NIL => Id];
    }
}
unsafe fn render_map_png(view: Id, b: NSRect, path: &str) {
    render_view_png(view, b, path, map_draw);
}
/// Draw a flipped custom view's drawRect into a PNG.
unsafe fn render_view_png(view: Id, b: NSRect, path: &str, draw: extern "C" fn(Id, Sel, NSRect)) {
    let rep = msg![Id; view, "bitmapImageRepForCachingDisplayInRect:", b => NSRect];
    let ctx = msg![Id; cls("NSGraphicsContext"), "graphicsContextWithBitmapImageRep:", rep => Id];
    msg![(); cls("NSGraphicsContext"), "saveGraphicsState"];
    let cg = msg![CG; ctx, "CGContext"];
    // the map view is flipped (y down); match that
    CGContextTranslateCTM(cg, 0.0, b.h);
    CGContextScaleCTM(cg, 1.0, -1.0);
    let fctx = msg![Id; cls("NSGraphicsContext"), "graphicsContextWithCGContext:flipped:", cg => CG, true => bool];
    msg![(); cls("NSGraphicsContext"), "setCurrentContext:", fctx => Id];
    // page background, so the PNG looks like the window
    msg![(); sys_color("windowBackgroundColor"), "setFill"];
    msg![(); cls("NSBezierPath"), "fillRect:", b => NSRect];
    draw(view, sel("drawRect:"), b);
    msg![(); cls("NSGraphicsContext"), "restoreGraphicsState"];
    let data = msg![Id; rep, "representationUsingType:properties:", 4usize => usize, msg![Id; cls("NSDictionary"), "dictionary"] => Id];
    msg![bool; data, "writeToFile:atomically:", ns(path) => Id, true => bool];
}

/// Health page with the made-up genotypes at the bottom of this file, then quit.
fn healthshot_start() {
    if std::env::var("G25_HEALTHSHOT").is_err() {
        return;
    }
    with_state(|s| s.health_items = Some(health::items(&demo())));
    if let Some(&c) = CONTROLLER.get() {
        unsafe { msg![(); c as Id, "performSelector:withObject:afterDelay:", sel("healthShot:") => Sel, NIL => Id, 0.5f64 => f64] }
    }
}
extern "C" fn on_healthshot(_this: Id, _cmd: Sel, _arg: Id) {
    let path = std::env::var("G25_HEALTHSHOT").unwrap_or_default();
    unsafe {
        let Some(w) = W.get() else { return };
        let v = w.health as Id;
        let total = with_state(|s| s.health_items.as_ref().map(|i| health_layout(i).1)).unwrap_or(600.0);
        let f = msg![NSRect; v, "frame"];
        msg![(); v, "setFrameSize:", NSSize { w: f.w, h: total } => NSSize];
        render_view_png(v, rect(0.0, 0.0, f.w, total), &path, health_draw);
        msg![(); msg![Id; cls("NSApplication"), "sharedApplication"], "terminate:", NIL => Id];
    }
}

/// qpAdm page: Suggest, Rotate, then Run model on the target (Hungarian by default); saves the
/// window and the full results list, then quits.
static QP_SHOT_STEP: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
fn qpshot_start() {
    if std::env::var("G25_QPSHOT").is_err() {
        return;
    }
    with_state(|s| s.page = 3);
    std::thread::spawn(|| loop {
        std::thread::sleep(std::time::Duration::from_millis(400));
        let ready = PACK.get().is_some() && !with_state(|s| s.qp_busy);
        if ready {
            if let Some(&c) = CONTROLLER.get() {
                unsafe { msg![(); c as Id, "performSelectorOnMainThread:withObject:waitUntilDone:", sel("qpShot:") => Sel, NIL => Id, true => bool] }
            }
            if QP_SHOT_STEP.load(std::sync::atomic::Ordering::Relaxed) >= 4 {
                return;
            }
        }
    });
}
extern "C" fn on_qpshot(_this: Id, _cmd: Sel, _arg: Id) {
    use std::sync::atomic::Ordering::Relaxed;
    unsafe {
        let Some(q) = QP.get() else { return };
        match QP_SHOT_STEP.fetch_add(1, Relaxed) {
            0 => {
                apply_layout();
                qp_fill_pickers();
                let mut t = std::env::var("G25_QPTARGET").unwrap_or_else(|_| "Hungarian".into());
                // "Name=A+B+C" makes a merged population of your own first
                if let Some((name, members)) = t.clone().split_once('=') {
                    let members: Vec<String> = members.split('+').map(|x| x.to_string()).collect();
                    let g25 = {
                        let cs: Vec<Vec<f64>> = members.iter().filter_map(|m| qp_coords(m)).collect();
                        (cs.len() == members.len()).then(|| (0..cs[0].len()).map(|d| cs.iter().map(|c| c[d]).sum::<f64>() / cs.len() as f64).collect())
                    };
                    if let Some(Ok(pack)) = PACK.get() {
                        if let Err(e) = qpadm::add_merged(pack, name, &members, None, g25) {
                            eprintln!("qpshot merge: {e}");
                        }
                    }
                    qp_fill_pickers();
                    t = name.to_string();
                }
                if std::env::var("G25_QPPIE").is_ok() {
                    QP_PIE.store(true, Relaxed);
                    msg![(); q.pie as Id, "setState:", 1isize => isize];
                }
                msg![(); q.target as Id, "setStringValue:", ns(&t) => Id];
                on_qp_suggest(NIL, sel("qpSuggest:"), NIL);
                on_qp_rotate(NIL, sel("qpRotate:"), NIL);
            }
            1 => {
                // keep a picture of the rotation table, then run the best rotated model
                let path = std::env::var("G25_QPSHOT").unwrap_or_default();
                let v = q.view as Id;
                let f = msg![NSRect; v, "frame"];
                render_view_png(v, rect(0.0, 0.0, f.w, f.h), &path.replace(".png", "_rotation.png"), qp_draw);
                let src = with_state(|s| s.qp.as_ref().and_then(|r| r.rotation.iter().find(|m| m.sources.len() >= 2).map(|m| m.sources.clone())));
                let src = std::env::var("G25_QPSOURCES").ok().map(|x| x.split(',').map(|s| s.to_string()).collect::<Vec<_>>()).or(src);
                if let Some(src) = src {
                    msg![(); q.sources as Id, "setString:", ns(&src.join("\n")) => Id];
                }
                on_qp_run(NIL, sel("qpRun:"), NIL);
            }
            2 => {
                let path = std::env::var("G25_QPSHOT").unwrap_or_default();
                let Some(w) = W.get() else { return };
                let wid = msg![isize; w.win as Id, "windowNumber"] as u32;
                // kCGWindowListOptionIncludingWindow, kCGWindowImageBoundsIgnoreFraming
                let img = CGWindowListCreateImage(NSRect { x: f64::INFINITY, y: f64::INFINITY, w: 0.0, h: 0.0 }, 8, wid, 1);
                let rep = msg![Id; msg![Id; cls("NSBitmapImageRep"), "alloc"], "initWithCGImage:", img => CG];
                let data = msg![Id; rep, "representationUsingType:properties:", 4usize => usize, msg![Id; cls("NSDictionary"), "dictionary"] => Id];
                msg![bool; data, "writeToFile:atomically:", ns(&path) => Id, true => bool];
                // the results list in full
                let v = q.view as Id;
                let f = msg![NSRect; v, "frame"];
                render_view_png(v, rect(0.0, 0.0, f.w, f.h), &path.replace(".png", "_results.png"), qp_draw);
                QP_SHOT_STEP.store(9, Relaxed);
                msg![(); msg![Id; cls("NSApplication"), "sharedApplication"], "terminate:", NIL => Id];
            }
            _ => {}
        }
    }
}

// made-up genotypes that show every kind of Health card (not a real person)
fn demo() -> RawGenotypes {
    let mut g = RawGenotypes::default();
    for (rs, gt) in [
        (6025u32, *b"CT"), (1799963, *b"GG"), (10757278, *b"GG"), (429358, *b"TC"), (7412, *b"CC"),
        (7903146, *b"CT"), (1061170, *b"TC"), (10490924, *b"GG"), (9939609, *b"AT"), (1800562, *b"GG"),
        (1799945, *b"CG"), (28929474, *b"CC"), (738409, *b"CG"), (2200733, *b"CC"), (10455872, *b"AA"),
        (2231142, *b"GT"), (3923809, *b"GA"), (16969968, *b"GA"), (2476601, *b"GG"), (3135388, *b"GG"),
        (10484554, *b"CC"), (4349859, *b"GG"), (2187668, *b"CT"), (7454108, *b"TT"),
        (4244285, *b"GA"), (12248560, *b"CC"), (4149056, *b"TC"), (1799853, *b"CC"), (1057910, *b"AA"),
        (9923231, *b"CT"), (1801133, *b"GA"),
        (762551, *b"AC"), (671, *b"GG"), (1229984, *b"CC"), (4481887, *b"AA"), (10427255, *b"CT"),
        (4988235, *b"AG"), (713598, *b"CG"), (10246939, *b"TC"),
    ] {
        g.insert(rs, gt);
    }
    g
}
