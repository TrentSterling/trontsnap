// Embeds TrontSnap's distinct rainbow icon into the exe (Explorer/taskbar/PE
// icon) and a Win32 manifest. The manifest is feature-gated:
//
//   default             -> asInvoker, uiAccess=false  (portable; `cargo run` works)
//   --features uiaccess -> asInvoker, uiAccess=true   (the installed build)
//
// WHY uiAccess IS BACK. v0.10 removed it on the belief that RegisterHotKey is
// never UIPI-blocked, so a plain Medium exe would always receive its hotkeys.
// That is true for combos WITH modifiers and FALSE for modifier-less ones: a
// bare PrtSc bind registered by a Medium process is not delivered while an
// ELEVATED window has focus. Proven by A/B on this machine 2026-07-23 (bare
// PrtSc and bare F9 blocked, Ctrl+Alt+F9 and Ctrl+PrtSc fine) and reconfirmed
// 2026-07-26 against Task Manager.
//
// uiAccess=true sets the token flag that bypasses UIPI, which restores bare
// PrtSc everywhere without elevating the process, so drag-out into Discord
// keeps working (confirmed live on the v0.9.0 build, which ran High yet still
// dragged out fine). Windows only GRANTS uiAccess when the exe is
// Authenticode-signed by a trusted-root cert AND lives in a secure location
// (%ProgramFiles%) — bootstrap.ps1 handles both. A uiAccess exe also cannot be
// launched via bare CreateProcess, which is exactly why it stays opt-in.

fn main() {
    if std::env::var_os("CARGO_CFG_WINDOWS").is_some() {
        let uiaccess = std::env::var_os("CARGO_FEATURE_UIACCESS").is_some();
        let mut res = winresource::WindowsResource::new();
        res.set_icon("assets/icon.ico");
        res.set_manifest(if uiaccess { MANIFEST_UIACCESS } else { MANIFEST_PLAIN });
        res.compile().expect("embed icon + manifest");
    }
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=assets/icon.ico");
}

const MANIFEST_PLAIN: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<assembly xmlns="urn:schemas-microsoft-com:asm.v1" manifestVersion="1.0">
  <trustInfo xmlns="urn:schemas-microsoft-com:asm.v3">
    <security>
      <requestedPrivileges>
        <requestedExecutionLevel level="asInvoker" uiAccess="false"/>
      </requestedPrivileges>
    </security>
  </trustInfo>
</assembly>"#;

const MANIFEST_UIACCESS: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<assembly xmlns="urn:schemas-microsoft-com:asm.v1" manifestVersion="1.0">
  <trustInfo xmlns="urn:schemas-microsoft-com:asm.v3">
    <security>
      <requestedPrivileges>
        <requestedExecutionLevel level="asInvoker" uiAccess="true"/>
      </requestedPrivileges>
    </security>
  </trustInfo>
</assembly>"#;
