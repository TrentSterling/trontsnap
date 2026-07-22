// Embeds TrontSnap's distinct rainbow icon into the exe (Explorer/taskbar/PE icon)
// and a Win32 manifest. The manifest is feature-gated:
//
//   default            -> asInvoker, uiAccess=false  (plain; `cargo run` works)
//   --features uiaccess -> asInvoker, uiAccess=true   (installer/release build)
//
// uiAccess=true lets our WH_KEYBOARD_LL hook receive keystrokes destined for
// ELEVATED windows (like TrontEQ) WITHOUT elevating TrontSnap itself, so it stays
// Medium integrity and drag-out keeps working. Windows only GRANTS uiAccess when
// the exe is Authenticode-signed by a trusted-root cert AND lives in a secure
// location (%ProgramFiles%) — the installer (bootstrap.ps1) handles both. A
// uiAccess exe also can't be launched via bare CreateProcess (`cargo run` would
// fail with ERROR_ELEVATION_REQUIRED), which is exactly why it's opt-in.

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
