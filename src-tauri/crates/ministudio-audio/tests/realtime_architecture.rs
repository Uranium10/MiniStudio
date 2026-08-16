//! Cheap architecture gates for the callback boundary.
//!
//! These do not replace runtime allocation instrumentation, but they prevent the most damaging
//! control/UI dependencies and blocking primitives from entering the explicit CPAL callback.

#[test]
fn audio_crate_has_no_tauri_or_window_dependency() {
    let manifest = include_str!("../Cargo.toml").to_ascii_lowercase();
    assert!(!manifest.contains("tauri"));
    assert!(!manifest.contains("wry"));
    assert!(!manifest.contains("webview"));
}

#[test]
fn explicit_cpal_callback_contains_no_blocking_control_primitive() {
    let source = include_str!("../src/audio/device.rs");
    let callback = between(source, "move |data: &mut [T], _| {", "move |_| {");
    for forbidden in [
        "Mutex",
        "RwLock",
        ".lock(",
        ".recv(",
        ".send(",
        "File::",
        "fs::",
        "println!",
        "eprintln!",
        "Command::",
    ] {
        assert!(
            !callback.contains(forbidden),
            "forbidden callback primitive `{forbidden}` entered the CPAL callback"
        );
    }
}

#[test]
fn audio_core_render_has_no_direct_io_or_lock() {
    let source = include_str!("../src/audio/engine.rs");
    let render = between(
        source,
        "pub fn render(&mut self, output: &mut [f32], frames: usize) {",
        "fn render_timeline(&mut self",
    );
    for forbidden in [".lock(", "File::", "fs::", "println!", "eprintln!"] {
        assert!(
            !render.contains(forbidden),
            "forbidden render primitive `{forbidden}` entered AudioCore::render"
        );
    }
}

#[test]
fn audio_graph_process_has_no_obvious_allocator_or_blocking_primitive() {
    let source = include_str!("../src/audio/graph.rs");
    let process = between(
        source,
        "pub fn process(\n        &mut self,",
        "/// Positions a newly built graph",
    );
    for forbidden in [
        ".lock(",
        "Mutex",
        "RwLock",
        ".recv(",
        "File::",
        "fs::",
        "println!",
        "eprintln!",
        ".collect(",
        "Vec::",
        "vec![",
        ".sort_by_key(",
        ".sort_by(",
    ] {
        assert!(
            !process.contains(forbidden),
            "forbidden realtime primitive `{forbidden}` entered AudioGraph::process"
        );
    }
}

fn between<'a>(source: &'a str, start: &str, end: &str) -> &'a str {
    let start = source.find(start).expect("callback start marker") + start.len();
    let end = source[start..].find(end).expect("callback end marker") + start;
    &source[start..end]
}
