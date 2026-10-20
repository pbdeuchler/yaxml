// Leave interpreter symbols to the loading Python (`-undefined dynamic_lookup`
// on macOS), so plain `cargo build` links the extension as maturin does.
fn main() {
    pyo3_build_config::add_extension_module_link_args();
}
