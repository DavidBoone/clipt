// `wayland_x11`: the Wayland and X11 desktops, which is every Unix but
// macOS (Linux, the BSDs). Cargo.toml's target tables can't use it, so they
// spell it out: cfg(all(unix, not(target_os = "macos"))).
fn main() {
    println!("cargo::rustc-check-cfg=cfg(wayland_x11)");
    let unix = std::env::var("CARGO_CFG_TARGET_FAMILY").is_ok_and(|f| f.split(',').any(|f| f == "unix"));
    if unix && std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("macos") {
        println!("cargo::rustc-cfg=wayland_x11");
    }
}
