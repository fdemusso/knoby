//! Embeds the app icon and Windows version info (shown by Explorer and Task Manager) into the executable.
//! The resource script is generated here so the version always follows `Cargo.toml`.

use std::{env, fs, path::PathBuf};

fn main() {
    println!("cargo:rerun-if-changed=assets/knoby.ico");
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=Cargo.toml");

    let var = |k: &str| env::var(k).unwrap();
    let (major, minor, patch) = (var("CARGO_PKG_VERSION_MAJOR"), var("CARGO_PKG_VERSION_MINOR"), var("CARGO_PKG_VERSION_PATCH"));
    let version = var("CARGO_PKG_VERSION");
    // Forward slashes: backslashes would need escaping inside .rc strings.
    let icon = PathBuf::from(var("CARGO_MANIFEST_DIR")).join("assets").join("knoby.ico");
    let icon = icon.to_string_lossy().replace('\\', "/");

    let rc = format!(
        r#"1 ICON "{icon}"

1 VERSIONINFO
 FILEVERSION {major},{minor},{patch},0
 PRODUCTVERSION {major},{minor},{patch},0
 FILEFLAGSMASK 0x3fL
 FILEFLAGS 0x0L
 FILEOS 0x40004L
 FILETYPE 0x1L
 FILESUBTYPE 0x0L
BEGIN
  BLOCK "StringFileInfo"
  BEGIN
    BLOCK "040904b0"
    BEGIN
      VALUE "FileDescription", "Knoby"
      VALUE "ProductName", "Knoby"
      VALUE "InternalName", "knoby"
      VALUE "OriginalFilename", "knoby.exe"
      VALUE "LegalCopyright", "Copyright (c) 2026 Flavio De Musso. MIT License."
      VALUE "FileVersion", "{version}"
      VALUE "ProductVersion", "{version}"
    END
  END
  BLOCK "VarFileInfo"
  BEGIN
    VALUE "Translation", 0x409, 1200
  END
END
"#
    );
    let path = PathBuf::from(var("OUT_DIR")).join("knoby.rc");
    fs::write(&path, rc).unwrap();
    embed_resource::compile(path, embed_resource::NONE).manifest_required().unwrap();
}
