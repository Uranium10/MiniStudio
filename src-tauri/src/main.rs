// Native executable entry point.
fn main() {
    if ministudio_lib::run_plugin_host_from_args() {
        return;
    }
    if ministudio_lib::run_plugin_probe_from_args() {
        return;
    }
    if ministudio_lib::run_midi_probe_from_args() {
        return;
    }
    ministudio_lib::run();
}
