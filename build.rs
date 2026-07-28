// Embeds TrontSnap's rainbow icon and an explicit asInvoker manifest.
//
// THERE IS ONLY ONE trontsnap.exe, and it is always Medium integrity. The
// portable download and the Program Files install ship the SAME binary; the only
// difference is whether trontsnap-hotkeys.exe is installed beside it.
//
// It deliberately never gets uiAccess, and the feature that used to switch that
// on is gone. uiAccess is granted by handing out a raised token that lands at
// HIGH integrity, and Windows blocks OLE drag-and-drop across an integrity
// boundary, so a uiAccess UI cannot drag a screenshot into Discord or a
// terminal. v0.14.0 shipped that and the breakage was immediate.
//
// The reason uiAccess was wanted (a bare PrtSc bind is not delivered to a Medium
// process while an elevated window has focus) is handled by hotkeyd/, a tiny
// uiAccess broker with no UI that owns the registration and relays presses over
// loopback. Privilege where it is needed, nowhere else.
//
// The manifest is stated explicitly rather than left to the default so that this
// decision is visible in the binary and in this file, instead of being an
// absence someone has to infer.

fn main() {
    if std::env::var_os("CARGO_CFG_WINDOWS").is_some() {
        let mut res = winresource::WindowsResource::new();
        res.set_icon("assets/icon.ico");
        res.set_manifest(MANIFEST);
        res.compile().expect("embed icon + manifest");
    }
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=assets/icon.ico");
}

const MANIFEST: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<assembly xmlns="urn:schemas-microsoft-com:asm.v1" manifestVersion="1.0">
  <trustInfo xmlns="urn:schemas-microsoft-com:asm.v3">
    <security>
      <requestedPrivileges>
        <requestedExecutionLevel level="asInvoker" uiAccess="false"/>
      </requestedPrivileges>
    </security>
  </trustInfo>
</assembly>"#;
