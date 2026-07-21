// napi-build wires up the linker so the cdylib resolves node's N-API symbols at
// load time (undefined at link time, provided by the node process). Mirrors every
// napi-rs addon's build script.
fn main() {
    napi_build::setup();
}
