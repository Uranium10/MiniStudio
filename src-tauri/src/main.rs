// Native executable entry point.
fn main() {
    if minidaw_lib::run_plugin_probe_from_args() {
        return;
    }
    minidaw_lib::run();
}
