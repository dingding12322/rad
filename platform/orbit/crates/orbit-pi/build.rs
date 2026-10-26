//! Build-time configuration for the in-app updater and the Windows binary.
//!
//! The public half of the release signing key is compiled in as
//! `ORBIT_UPDATE_PUBLIC_KEY` so every platform verifies the same EdDSA
//! signatures. Prefer the `ORBIT_UPDATE_PUBLIC_KEY` environment variable; when
//! it is absent, fall back to `SUPublicEDKey` in a bundled `Info.plist` (the
//! same key macOS tooling reads), and finally to an empty string, which leaves
//! the updater dormant.
//!
//! On Windows the binary also gets an embedded icon, version metadata, and the
//! per-monitor-DPI manifest (see [`embed_windows_resources`]).

use std::path::PathBuf;

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-env-changed=ORBIT_UPDATE_PUBLIC_KEY");
    track_locales();
    track_assets();

    let key = public_key();
    println!("cargo:rustc-env=ORBIT_UPDATE_PUBLIC_KEY={key}");

    embed_windows_resources();
}

/// Make an added or edited asset rebuild the binary.
///
/// `include_dir!` reads `assets/` at compile time, and Cargo has no idea the
/// directory is an input: without this, dropping in a new icon and rebuilding
/// silently keeps the old set, so a path that resolves in a fresh clone does not
/// resolve here (and the reverse). One directory-level dependency covers
/// `dev-assets/` too — it is `include_dir!`-ed the same way on debug macOS
/// builds.
fn track_assets() {
    println!("cargo:rerun-if-changed=assets");
    println!("cargo:rerun-if-changed=dev-assets");
}

/// Make translation edits rebuild the binary.
///
/// `rust_i18n::i18n!` bakes `locales/*.yml` in at compile time but never tells
/// Cargo those files are inputs, so a locale-only change (a fixed translation,
/// or a key added after the last code change) is silently dropped and the
/// binary keeps serving the old strings — a key with no embedded value renders
/// its raw name. Hash the locale files into a `rustc-env` value: when the hash
/// changes, Cargo sees different build-script output and recompiles the crate,
/// re-running the macro against the current files.
fn track_locales() {
    use std::hash::{Hash, Hasher};

    println!("cargo:rerun-if-changed=locales");
    let root = PathBuf::from(
        std::env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR is always set"),
    );
    let mut paths: Vec<PathBuf> = std::fs::read_dir(root.join("locales"))
        .into_iter()
        .flatten()
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "yml"))
        .collect();
    paths.sort();

    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    for path in &paths {
        println!("cargo:rerun-if-changed={}", path.display());
        if let Ok(bytes) = std::fs::read(path) {
            bytes.hash(&mut hasher);
        }
    }
    println!("cargo:rustc-env=ORBIT_I18N_HASH={:016x}", hasher.finish());
}

fn public_key() -> String {
    if let Ok(value) = std::env::var("ORBIT_UPDATE_PUBLIC_KEY") {
        let value = value.trim().to_owned();
        if !value.is_empty() {
            return value;
        }
    }

    const PLIST: &str = "resources/Info.plist";
    const MARKER: &str = "<key>SUPublicEDKey</key>";
    if !std::path::Path::new(PLIST).exists() {
        return String::new();
    }
    println!("cargo:rerun-if-changed={PLIST}");

    let Ok(plist) = std::fs::read_to_string(PLIST) else {
        return String::new();
    };
    plist
        .split_once(MARKER)
        .and_then(|(_, rest)| rest.split_once("<string>"))
        .and_then(|(_, rest)| rest.split_once("</string>"))
        .map(|(value, _)| value.trim().to_owned())
        .unwrap_or_default()
}

/// Embed the application icon, a `VERSIONINFO` block, and the DPI manifest into
/// `orbit-pi.exe`.
///
/// The resource script is generated into `OUT_DIR` so the version and the
/// absolute icon paths are baked in, and `embed-resource` compiles it with the
/// MSVC toolchain's `rc.exe` (which it locates itself). This is a no-op on
/// every non-Windows target: on macOS/Linux [`embed_resource::compile`] returns
/// `NotWindows`, which `manifest_optional()` accepts.
fn embed_windows_resources() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }

    let root = PathBuf::from(
        std::env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR is always set"),
    );
    let resources = root.join("resources");
    let icons = root.join("../../assets/icons");

    println!(
        "cargo:rerun-if-changed={}",
        icons.join("icon.ico").display()
    );
    println!(
        "cargo:rerun-if-changed={}",
        resources.join("windows.manifest").display()
    );

    // VERSIONINFO wants four comma-separated numbers; a bare `0.0.2` is not a
    // valid FOUR-part version, so zero-pad it.
    let mut parts = [0u16; 4];
    for (slot, part) in parts
        .iter_mut()
        .zip(env!("CARGO_PKG_VERSION").split(['.', '-']))
    {
        *slot = part.parse().unwrap_or(0);
    }
    let file_version = format!("{},{},{},{}", parts[0], parts[1], parts[2], parts[3]);
    let version = env!("CARGO_PKG_VERSION");

    let out_dir = PathBuf::from(std::env::var("OUT_DIR").expect("OUT_DIR is always set"));
    let script = out_dir.join("orbit-pi.rc");
    // The leading whitespace RC sees is the source indentation; the resource
    // compiler ignores it. Inline (rather than a checked-in `.rc`) so the
    // absolute icon paths and the package version need no macro plumbing.
    let body = format!(
        r#"// Generated by build.rs - Windows resources for orbit-pi.exe.
1 ICON "icon.ico"
1 24 "windows.manifest"

1 VERSIONINFO
FILEVERSION {file_version}
PRODUCTVERSION {file_version}
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
            VALUE "CompanyName", "Orbit"
            VALUE "FileDescription", "Orbit Pi - native workbench for the pi coding agent"
            VALUE "FileVersion", "{version}"
            VALUE "InternalName", "orbit-pi"
            VALUE "LegalCopyright", "Apache-2.0"
            VALUE "OriginalFilename", "orbit-pi.exe"
            VALUE "ProductName", "Orbit Pi"
            VALUE "ProductVersion", "{version}"
        END
    END
    BLOCK "VarFileInfo"
    BEGIN
        VALUE "Translation", 0x409, 1200
    END
END
"#,
        file_version = file_version,
        version = version,
    );
    std::fs::write(&script, body).expect("failed to write the generated Windows resource script");

    embed_resource::compile(
        &script,
        embed_resource::ParamsIncludeDirs([icons, resources]),
    )
    .manifest_optional()
    .expect("failed to embed Windows resources into orbit-pi.exe");
}
