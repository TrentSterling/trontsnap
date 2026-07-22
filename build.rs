// Embeds TrontSnap's distinct rainbow icon into the exe (Explorer/taskbar/PE
// icon). No manifest is set: the default is asInvoker, uiAccess=false, which
// is exactly the portable/Medium-integrity behavior we want, so there's
// nothing to opt in or out of.

fn main() {
    if std::env::var_os("CARGO_CFG_WINDOWS").is_some() {
        let mut res = winresource::WindowsResource::new();
        res.set_icon("assets/icon.ico");
        res.compile().expect("embed icon");
    }
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=assets/icon.ico");
}
