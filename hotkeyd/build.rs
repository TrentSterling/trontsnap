// Embeds the uiAccess manifest into trontsnap-hotkeys.exe.
//
// uiAccess=true is what makes a RegisterHotKey bind survive an ELEVATED window
// having focus. Windows only GRANTS it to an exe that is Authenticode-signed by
// a trusted-root cert AND lives in a secure location (%ProgramFiles%), so this
// binary is only useful once installed; bootstrap.ps1 does both.
//
// This is a SEPARATE CRATE from trontsnap precisely because of this file: a build
// script emits one resource for the whole crate, so one crate cannot produce both
// a uiAccess binary and a plain one. The main UI must stay non-uiAccess, because
// uiAccess is granted via a raised token that lands at HIGH integrity, and
// Windows blocks drag-and-drop across an integrity boundary. Splitting the two
// is the only way to have working drag-out AND working bare PrtSc over elevated
// windows.

fn main() {
    if std::env::var_os("CARGO_CFG_WINDOWS").is_some() {
        let mut res = winresource::WindowsResource::new();
        res.set_manifest(MANIFEST_UIACCESS);
        res.compile().expect("embed uiAccess manifest");
    }
    println!("cargo:rerun-if-changed=build.rs");
}

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
