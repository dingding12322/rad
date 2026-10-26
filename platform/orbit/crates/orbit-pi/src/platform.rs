//! Platform helpers for detecting installed folder-capable apps and opening a
//! workspace path in one of them (editors, terminals, the file manager).
//!
//! macOS resolves apps through Launch Services bundle ids; Windows probes the
//! usual install locations and `PATH` and launches the executable directly.
//! Both feed the same [`ExternalApp`] menu the header renders.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use gpui::{Image, SharedString, TitlebarOptions, Window};
// `point`/`px` place the macOS traffic lights; no other platform takes a
// position, so they stay behind the same gate as that titlebar.
#[cfg(target_os = "macos")]
use gpui::{point, px};
use serde::{Deserialize, Serialize};

/// A folder-capable application the header's "open in" control can target,
/// resolved against what is installed on this machine.
#[derive(Clone)]
pub struct ExternalApp {
    /// Stable identifier persisted as the user's preferred target.
    pub id: &'static str,
    pub label: &'static str,
    /// The launch target that resolved here: a Launch Services bundle id on
    /// macOS, the executable's full path on Windows. Handed back to
    /// [`open_path_in_app`] together with the [`ExternalApp`] so the platform
    /// can also recover any app-specific arguments.
    pub target: String,
    pub icon: Arc<Image>,
}

/// Known folder-capable apps in menu order — editors, the file manager,
/// terminals, IDEs. An entry lists every bundle id it ships under; the first
/// installed one wins.
#[cfg(target_os = "macos")]
const TERMY_BUNDLE_ID: &str = "com.lassevestergaard.termy";

#[cfg(target_os = "macos")]
const OPEN_IN_CATALOG: &[(&str, &str, &[&str])] = &[
    ("vscode", "VS Code", &["com.microsoft.VSCode"]),
    ("cursor", "Cursor", &["com.todesktop.230313mzl4w4u92"]),
    ("zed", "Zed", &["dev.zed.Zed", "dev.zed.Zed-Preview"]),
    ("finder", "Finder", &["com.apple.finder"]),
    ("terminal", "Terminal", &["com.apple.Terminal"]),
    ("termy", "Termy", &[TERMY_BUNDLE_ID]),
    ("iterm2", "iTerm2", &["com.googlecode.iterm2"]),
    ("kitty", "Kitty", &["net.kovidgoyal.kitty"]),
    ("ghostty", "Ghostty", &["com.mitchellh.ghostty"]),
    ("warp", "Warp", &["dev.warp.Warp-Stable", "dev.warp.Warp"]),
    ("xcode", "Xcode", &["com.apple.dt.Xcode"]),
    (
        "rider",
        "Rider",
        &["com.jetbrains.rider", "com.jetbrains.rider-EAP"],
    ),
    (
        "android-studio",
        "Android Studio",
        &["com.google.android.studio"],
    ),
];

/// Side of the square app-icon PNG the "open in" menu embeds, in pixels.
///
/// The menu draws app icons in an 18px slot (`open_in::APP_ICON`), so 2× the
/// slot matches what a Retina window blits 1:1. That matters because gpui
/// paints a raster image with a single bilinear pass and no mipmaps, so the
/// source has to arrive already downscaled — see [`app_icon_png`].
#[cfg(target_os = "macos")]
const APP_ICON_PX: usize = 36;

/// Rasterize an app bundle's icon to a `px`×`px` PNG.
///
/// `NSWorkspace`'s image carries one bitmap representation per icon size — up
/// to 2048×2048 — while `-setSize:` only changes the logical size and
/// `-TIFFRepresentation` hands back the *largest* rep (a 1024×1024 bitmap for
/// every app in the catalog). Encoding that unchanged is what the menu used to
/// embed, and a single bilinear tap then had to squeeze it into a 14px slot on
/// the GPU. Redrawing into a bitmap of exactly `px` pixels instead resamples
/// once with AppKit's high-quality filter, off the renderer's hot path — and
/// shrinks each embedded icon from ~1 MB to ~2 KB.
#[cfg(target_os = "macos")]
fn app_icon_png(application_path: &objc2_foundation::NSString, px: usize) -> Option<Vec<u8>> {
    use objc2::AllocAnyThread;
    use objc2_app_kit::{
        NSBitmapImageFileType, NSBitmapImageRep, NSDeviceRGBColorSpace, NSGraphicsContext,
        NSImageInterpolation, NSWorkspace,
    };
    use objc2_foundation::{NSDictionary, NSPoint, NSRect, NSSize};

    let image = NSWorkspace::sharedWorkspace().iconForFile(application_path);

    let side = px as isize;
    // SAFETY: `planes` is null so AppKit allocates the buffer, and the rest
    // describes a standard 8-bit RGBA, non-planar bitmap.
    let bitmap = unsafe {
        NSBitmapImageRep::initWithBitmapDataPlanes_pixelsWide_pixelsHigh_bitsPerSample_samplesPerPixel_hasAlpha_isPlanar_colorSpaceName_bytesPerRow_bitsPerPixel(
            NSBitmapImageRep::alloc(),
            std::ptr::null_mut(),
            side,
            side,
            8,
            4,
            true,
            false,
            NSDeviceRGBColorSpace,
            0,
            0,
        )
    }?;
    let context = NSGraphicsContext::graphicsContextWithBitmapImageRep(&bitmap)?;
    // AppKit's own resampler beats the renderer's bilinear tap on icon art.
    context.setImageInterpolation(NSImageInterpolation::High);
    context.setShouldAntialias(true);

    NSGraphicsContext::saveGraphicsState_class();
    NSGraphicsContext::setCurrentContext(Some(&context));
    image.drawInRect(NSRect::new(
        NSPoint::ZERO,
        NSSize::new(px as f64, px as f64),
    ));
    NSGraphicsContext::restoreGraphicsState_class();

    let properties = NSDictionary::new();
    // SAFETY: the properties dictionary is empty and `bitmap` is live.
    let png_data = unsafe {
        bitmap.representationUsingType_properties(NSBitmapImageFileType::PNG, &properties)
    }?;
    // SAFETY: `png_data` is a live `NSData`.
    let bytes = unsafe { png_data.as_bytes_unchecked() };
    (!bytes.is_empty()).then(|| bytes.to_vec())
}

/// Resolve which catalog apps are installed, with their icons.
#[cfg(target_os = "macos")]
pub fn detect_open_in_apps() -> Vec<ExternalApp> {
    use objc2_app_kit::NSWorkspace;
    use objc2_foundation::NSString;

    let workspace = NSWorkspace::sharedWorkspace();
    OPEN_IN_CATALOG
        .iter()
        .filter_map(|&(id, label, bundle_ids)| {
            bundle_ids.iter().find_map(|&bundle_id| {
                let application_url = workspace
                    .URLForApplicationWithBundleIdentifier(&NSString::from_str(bundle_id))?;
                let application_path = application_url.path()?;
                let bytes = app_icon_png(&application_path, APP_ICON_PX)?;
                Some(ExternalApp {
                    id,
                    label,
                    target: bundle_id.to_string(),
                    icon: Arc::new(Image::from_bytes(gpui::ImageFormat::Png, bytes)),
                })
            })
        })
        .collect()
}

/// Known folder-capable apps on Windows, in menu order. Each app lists the
/// executables it normally installs as, probe paths first and `PATH` names
/// second; `pre_args` are arguments that must precede the folder path (Windows
/// Terminal's `-d`). Editors take the folder path directly.
#[cfg(windows)]
mod windows_open_in {
    use std::ffi::c_void;
    use std::os::windows::ffi::OsStrExt as _;
    use std::path::{Path, PathBuf};
    use std::sync::Arc;

    use gpui::Image;
    use windows_sys::Win32::Graphics::Gdi::{
        CreateCompatibleDC, DeleteDC, DeleteObject, GetDIBits, GetObjectW, BITMAP, BITMAPINFO,
        BITMAPINFOHEADER, BI_RGB, DIB_RGB_COLORS, HBITMAP, HDC,
    };
    use windows_sys::Win32::UI::Shell::{SHGetFileInfoW, SHFILEINFOW, SHGFI_ICON};
    use windows_sys::Win32::UI::WindowsAndMessaging::{DestroyIcon, GetIconInfo, HICON, ICONINFO};

    use super::ExternalApp;

    struct WindowsApp {
        id: &'static str,
        label: &'static str,
        paths: &'static [&'static str],
        commands: &'static [&'static str],
        pre_args: &'static [&'static str],
    }

    /// The shell's icon APIs can race for the same file — a second concurrent
    /// caller may see an icon the first one is about to free — so extraction is
    /// serialized. Detection runs once, off-thread, so the lock is never hot.
    static ICON_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    const CATALOG: &[WindowsApp] = &[
        WindowsApp {
            id: "vscode",
            label: "VS Code",
            paths: &[
                "%LOCALAPPDATA%\\Programs\\Microsoft VS Code\\Code.exe",
                "%ProgramFiles%\\Microsoft VS Code\\Code.exe",
                "%ProgramFiles(x86)%\\Microsoft VS Code\\Code.exe",
            ],
            commands: &["Code.exe"],
            pre_args: &[],
        },
        WindowsApp {
            id: "cursor",
            label: "Cursor",
            paths: &[
                "%LOCALAPPDATA%\\Programs\\cursor\\Cursor.exe",
                "%LOCALAPPDATA%\\Programs\\Cursor\\Cursor.exe",
                "%ProgramFiles%\\Cursor\\Cursor.exe",
            ],
            commands: &["Cursor.exe"],
            pre_args: &[],
        },
        WindowsApp {
            id: "zed",
            label: "Zed",
            paths: &[
                "%LOCALAPPDATA%\\Programs\\Zed\\Zed.exe",
                "%ProgramFiles%\\Zed\\Zed.exe",
            ],
            commands: &["Zed.exe", "zed.exe"],
            pre_args: &[],
        },
        WindowsApp {
            id: "explorer",
            label: "File Explorer",
            paths: &["%WINDIR%\\explorer.exe"],
            commands: &["explorer.exe"],
            pre_args: &[],
        },
        WindowsApp {
            id: "terminal",
            label: "Windows Terminal",
            paths: &["%LOCALAPPDATA%\\Microsoft\\WindowsApps\\wt.exe"],
            commands: &["wt.exe"],
            pre_args: &["-d"],
        },
        WindowsApp {
            id: "rider",
            label: "Rider",
            paths: &[
                "%LOCALAPPDATA%\\Programs\\Rider\\bin\\rider64.exe",
                "%ProgramFiles%\\JetBrains\\JetBrains Rider\\bin\\rider64.exe",
            ],
            commands: &["rider64.exe"],
            pre_args: &[],
        },
        WindowsApp {
            id: "android-studio",
            label: "Android Studio",
            paths: &[
                "%ProgramFiles%\\Android\\Android Studio\\bin\\studio64.exe",
                "%LOCALAPPDATA%\\Programs\\Android Studio\\bin\\studio64.exe",
            ],
            commands: &["studio64.exe"],
            pre_args: &[],
        },
    ];

    /// Resolve the installed apps with their icons. Runs off-thread.
    pub(super) fn detect() -> Vec<ExternalApp> {
        CATALOG
            .iter()
            .filter_map(|app| {
                let exe = resolve(app)?;
                Some(ExternalApp {
                    id: app.id,
                    label: app.label,
                    target: exe.to_string_lossy().into_owned(),
                    icon: icon_image(&exe),
                })
            })
            .collect()
    }

    /// Open `path` in the app, using the catalog entry's leading arguments
    /// (Windows Terminal needs `-d` before the folder).
    pub(super) fn open(path: &Path, app: &ExternalApp) {
        let Some(entry) = CATALOG.iter().find(|entry| entry.id == app.id) else {
            return;
        };
        let mut command = std::process::Command::new(&app.target);
        command.args(entry.pre_args).arg(path);
        orbit_rpc::hide_console(&mut command);
        let _ = command.spawn();
    }

    fn resolve(app: &WindowsApp) -> Option<PathBuf> {
        app.paths
            .iter()
            .filter_map(|raw| expand_env(raw))
            .find(|path| path.is_file())
            .or_else(|| app.commands.iter().find_map(|command| which(command)))
    }

    /// Expand the `%VAR%` segments a catalog path may contain.
    pub(super) fn expand_env(raw: &str) -> Option<PathBuf> {
        let mut out = String::new();
        let mut rest = raw;
        while let Some(start) = rest.find('%') {
            out.push_str(&rest[..start]);
            let after = &rest[start + 1..];
            let end = after.find('%')?;
            out.push_str(&env_var(&after[..end])?);
            rest = &after[end + 1..];
        }
        out.push_str(rest);
        Some(PathBuf::from(out))
    }

    /// Environment lookup that tolerates the casing a user typed. Windows
    /// variable names are case-insensitive; `std::env::var` is not.
    fn env_var(name: &str) -> Option<String> {
        if let Ok(value) = std::env::var(name) {
            return Some(value);
        }
        std::env::vars().find_map(|(key, value)| key.eq_ignore_ascii_case(name).then_some(value))
    }

    /// First executable named `command` on `PATH`, without spawning `where`.
    fn which(command: &str) -> Option<PathBuf> {
        let path = std::env::var_os("PATH")?;
        std::env::split_paths(&path)
            .map(|dir| dir.join(command))
            .find(|candidate| candidate.is_file())
    }

    /// The executable's shell icon as a 32px PNG, or a neutral placeholder for
    /// the rare file whose icon the shell cannot hand back.
    fn icon_image(exe: &Path) -> Arc<Image> {
        let bytes = extract_icon_png(exe)
            .or_else(fallback_png)
            .expect("a fallback icon always encodes");
        Arc::new(Image::from_bytes(gpui::ImageFormat::Png, bytes))
    }

    /// Pull the executable's icon out of the shell and re-encode it as PNG.
    ///
    /// The shell's icon cache can transiently refuse a cold request, so a
    /// failed extraction is retried once. Detection runs off-thread anyway, so
    /// the extra attempt is invisible.
    pub(super) fn extract_icon_png(exe: &Path) -> Option<Vec<u8>> {
        let _guard = ICON_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        (0..2).find_map(|_| extract_icon_png_once(exe))
    }

    fn extract_icon_png_once(exe: &Path) -> Option<Vec<u8>> {
        let wide: Vec<u16> = exe
            .as_os_str()
            .encode_wide()
            .chain(std::iter::once(0))
            .collect();
        // SAFETY: `wide` is a NUL-terminated UTF-16 path and `info` is a valid
        // `SHFILEINFOW` for the shell to fill. `SHGFI_ICON` hands back an HICON
        // (32px, since large-icon is the default) that we own and destroy.
        unsafe {
            let mut info: SHFILEINFOW = std::mem::zeroed();
            let result = SHGetFileInfoW(
                wide.as_ptr(),
                0,
                &mut info,
                std::mem::size_of::<SHFILEINFOW>() as u32,
                SHGFI_ICON,
            );
            if result == 0 || info.hIcon.is_null() {
                return None;
            }
            let png = icon_to_png(info.hIcon);
            DestroyIcon(info.hIcon);
            png
        }
    }

    /// Convert an `HICON` into PNG bytes via its two GDI bitmaps.
    ///
    /// SAFETY: `hicon` must be a valid icon handle owned by the caller.
    unsafe fn icon_to_png(hicon: HICON) -> Option<Vec<u8>> {
        let mut info: ICONINFO = std::mem::zeroed();
        if unsafe { GetIconInfo(hicon, &mut info) } == 0 {
            return None;
        }
        let png = unsafe { bitmap_to_png(info.hbmColor, info.hbmMask) };
        // `GetIconInfo` hands us ownership of both bitmaps.
        for bitmap in [info.hbmColor, info.hbmMask] {
            if !bitmap.is_null() {
                unsafe { DeleteObject(bitmap) };
            }
        }
        png
    }

    /// SAFETY: `color` (and `mask`, when non-null) must be valid GDI bitmaps.
    unsafe fn bitmap_to_png(color: HBITMAP, mask: HBITMAP) -> Option<Vec<u8>> {
        let mut bitmap: BITMAP = unsafe { std::mem::zeroed() };
        if unsafe {
            GetObjectW(
                color,
                std::mem::size_of::<BITMAP>() as i32,
                &mut bitmap as *mut _ as *mut c_void,
            )
        } == 0
        {
            return None;
        }
        let width = bitmap.bmWidth;
        let height = bitmap.bmHeight;
        if width <= 0 || height <= 0 {
            return None;
        }
        let stride = width as usize * 4;
        let length = stride * height as usize;

        // Ask for 32-bit BGRA; a positive `biHeight` means the rows come back
        // bottom-up, so they are flipped below.
        let mut header: BITMAPINFO = unsafe { std::mem::zeroed() };
        header.bmiHeader.biSize = std::mem::size_of::<BITMAPINFOHEADER>() as u32;
        header.bmiHeader.biWidth = width;
        header.bmiHeader.biHeight = height;
        header.bmiHeader.biPlanes = 1;
        header.bmiHeader.biBitCount = 32;
        header.bmiHeader.biCompression = BI_RGB;

        let dc: HDC = unsafe { CreateCompatibleDC(std::ptr::null_mut()) };
        if dc.is_null() {
            return None;
        }
        let mut pixels = vec![0u8; length];
        let lines = unsafe {
            GetDIBits(
                dc,
                color,
                0,
                height as u32,
                pixels.as_mut_ptr() as *mut c_void,
                &mut header,
                DIB_RGB_COLORS,
            )
        };
        if lines == 0 {
            unsafe { DeleteDC(dc) };
            return None;
        }

        // A 32-bit icon from before the alpha channel was common stores its
        // transparency in the 1-bit mask instead; with no alpha set, apply it.
        let has_alpha = pixels.chunks_exact(4).any(|pixel| pixel[3] != 0);
        if !has_alpha && !mask.is_null() {
            unsafe { apply_mask(dc, mask, width, height, stride, &mut pixels) };
        }
        unsafe { DeleteDC(dc) };

        // BGRA bottom-up -> RGBA top-down.
        let mut rgba = vec![0u8; length];
        for y in 0..height as usize {
            let source = (height as usize - 1 - y) * stride;
            let target = y * stride;
            for x in 0..width as usize {
                let s = source + x * 4;
                let t = target + x * 4;
                rgba[t] = pixels[s + 2];
                rgba[t + 1] = pixels[s + 1];
                rgba[t + 2] = pixels[s];
                rgba[t + 3] = pixels[s + 3];
            }
        }

        let image = image::RgbaImage::from_raw(width as u32, height as u32, rgba)?;
        let mut out = Vec::new();
        image
            .write_to(&mut std::io::Cursor::new(&mut out), image::ImageFormat::Png)
            .ok()?;
        Some(out)
    }

    /// OR the icon's 1-bit mask into the alpha channel: a set mask bit means
    /// the pixel is transparent.
    ///
    /// SAFETY: `dc` must be a valid DC and `mask` a valid 1-bit GDI bitmap.
    unsafe fn apply_mask(
        dc: HDC,
        mask: HBITMAP,
        width: i32,
        height: i32,
        stride: usize,
        pixels: &mut [u8],
    ) {
        let mask_stride = (width as usize).div_ceil(32) * 4;
        let mut header: BITMAPINFO = unsafe { std::mem::zeroed() };
        header.bmiHeader.biSize = std::mem::size_of::<BITMAPINFOHEADER>() as u32;
        header.bmiHeader.biWidth = width;
        header.bmiHeader.biHeight = height;
        header.bmiHeader.biPlanes = 1;
        header.bmiHeader.biBitCount = 1;
        header.bmiHeader.biCompression = BI_RGB;
        let mut bits = vec![0u8; mask_stride * height as usize];
        if unsafe {
            GetDIBits(
                dc,
                mask,
                0,
                height as u32,
                bits.as_mut_ptr() as *mut c_void,
                &mut header,
                DIB_RGB_COLORS,
            )
        } == 0
        {
            return;
        }
        for y in 0..height as usize {
            let row = (height as usize - 1 - y) * mask_stride;
            for x in 0..width as usize {
                let transparent = (bits[row + x / 8] >> (7 - (x % 8))) & 1 == 1;
                pixels[y * stride + x * 4 + 3] = if transparent { 0 } else { 255 };
            }
        }
    }

    /// A neutral inset square for an app whose real icon could not be read.
    fn fallback_png() -> Option<Vec<u8>> {
        const SIZE: u32 = 32;
        const INSET: u32 = 5;
        let mut image = image::RgbaImage::new(SIZE, SIZE);
        let ink = image::Rgba([0x8a, 0x8f, 0x98, 0xff]);
        for (x, y, pixel) in image.enumerate_pixels_mut() {
            if x >= INSET && x < SIZE - INSET && y >= INSET && y < SIZE - INSET {
                *pixel = ink;
            }
        }
        let mut out = Vec::new();
        image
            .write_to(&mut std::io::Cursor::new(&mut out), image::ImageFormat::Png)
            .ok()?;
        Some(out)
    }
}

#[cfg(windows)]
pub fn detect_open_in_apps() -> Vec<ExternalApp> {
    windows_open_in::detect()
}

#[cfg(not(any(target_os = "macos", windows)))]
pub fn detect_open_in_apps() -> Vec<ExternalApp> {
    Vec::new()
}

/// Open `path` in the application `app`, activating it. macOS resolves the
/// bundle id through Launch Services, which delivers the open asynchronously;
/// Windows starts the executable directly.
#[cfg(target_os = "macos")]
pub fn open_path_in_app(path: &Path, app: &ExternalApp) {
    use objc2_app_kit::{NSWorkspace, NSWorkspaceOpenConfiguration};
    use objc2_foundation::{NSArray, NSString, NSURL};

    let bundle_id = app.target.as_str();
    let workspace = NSWorkspace::sharedWorkspace();
    let Some(application_url) =
        workspace.URLForApplicationWithBundleIdentifier(&NSString::from_str(bundle_id))
    else {
        return;
    };
    let url = if bundle_id == TERMY_BUNDLE_ID {
        let Some(url) = NSURL::URLWithString(&NSString::from_str(&termy_open_url(path))) else {
            return;
        };
        url
    } else {
        NSURL::fileURLWithPath(&NSString::from_str(&path.to_string_lossy()))
    };
    workspace.openURLs_withApplicationAtURL_configuration_completionHandler(
        &NSArray::from_retained_slice(&[url]),
        &application_url,
        &NSWorkspaceOpenConfiguration::configuration(),
        None,
    );
}

#[cfg(target_os = "macos")]
fn termy_open_url(path: &Path) -> String {
    let mut url = url::Url::parse("termy://new").expect("static Termy URL should be valid");
    url.query_pairs_mut()
        .append_pair("dir", &path.to_string_lossy());
    url.into()
}

#[cfg(windows)]
pub fn open_path_in_app(path: &Path, app: &ExternalApp) {
    windows_open_in::open(path, app);
}

#[cfg(not(any(target_os = "macos", windows)))]
pub fn open_path_in_app(_: &Path, _: &ExternalApp) {}

/// Reveal `path` in the OS file manager, selecting it. Used by the skills
/// page to open a skill's directory.
#[cfg(target_os = "macos")]
pub fn reveal_in_file_manager(path: &Path) {
    let _ = std::process::Command::new("/usr/bin/open")
        .arg("-R")
        .arg(path)
        .spawn();
}

#[cfg(not(target_os = "macos"))]
pub fn reveal_in_file_manager(path: &Path) {
    #[cfg(target_os = "windows")]
    {
        // `explorer /select,<path>` needs the comma but no separating space;
        // it is also the only way Explorer highlights the file itself.
        let _ = std::process::Command::new("explorer")
            .arg(format!("/select,{}", path.display()))
            .spawn();
    }
    #[cfg(not(target_os = "windows"))]
    {
        // Most Linux desktops have no "select" verb; open the containing
        // folder.
        let target = path.parent().unwrap_or(path);
        let _ = std::process::Command::new("xdg-open").arg(target).spawn();
    }
}

/// Whether [`trash_path`] moves a file to the OS trash (recoverable) or
/// deletes it outright. The Explorer's delete confirmation is worded from
/// this, so it never promises a recovery the platform cannot deliver.
pub const TRASH_IS_RECOVERABLE: bool = cfg!(target_os = "macos");

/// Delete `path`, preferring the OS trash so a mistake is recoverable.
///
/// macOS uses `NSFileManager`'s `trashItemAtURL:` — the same move Finder's
/// **Move to Trash** performs, including collision-renaming and restore.
/// Windows/Linux are best-effort and fall through to a permanent delete; the
/// caller checks [`TRASH_IS_RECOVERABLE`] before promising otherwise.
#[cfg(target_os = "macos")]
pub fn trash_path(path: &Path) -> std::io::Result<()> {
    use objc2_foundation::{NSFileManager, NSString, NSURL};

    let url = NSURL::fileURLWithPath(&NSString::from_str(&path.to_string_lossy()));
    NSFileManager::defaultManager()
        .trashItemAtURL_resultingItemURL_error(&url, None)
        .map_err(|error| std::io::Error::other(error.localizedDescription().to_string()))
}

#[cfg(not(target_os = "macos"))]
pub fn trash_path(path: &Path) -> std::io::Result<()> {
    if path.is_dir() {
        std::fs::remove_dir_all(path)
    } else {
        std::fs::remove_file(path)
    }
}

/// Open `path` in the OS default application (the user's editor for a
/// `SKILL.md`). Used by the skills page's **Open SKILL.md** action.
#[cfg(target_os = "macos")]
pub fn open_path_default(path: &Path) {
    let _ = std::process::Command::new("/usr/bin/open")
        .arg(path)
        .spawn();
}

#[cfg(not(target_os = "macos"))]
pub fn open_path_default(path: &Path) {
    // Windows has no `xdg-open`; the shell's `start` verb is the equivalent
    // (`""` is the window title `start` would otherwise read as the path).
    #[cfg(target_os = "windows")]
    {
        let mut command = std::process::Command::new("cmd");
        command.args(["/C", "start", ""]).arg(path);
        orbit_rpc::hide_console(&mut command);
        let _ = command.spawn();
    }
    #[cfg(not(target_os = "windows"))]
    let _ = std::process::Command::new("xdg-open").arg(path).spawn();
}

/// The user's home directory — the root of both pi's store (`~/.pi`) and
/// Orbit's own (`~/.orbit-pi`).
///
/// `HOME` is the POSIX answer and the one macOS and Linux always set. Windows
/// sets it only inside a POSIX-ish shell: a process started from Explorer or
/// PowerShell has `USERPROFILE` (and, on domain/legacy setups, `HOMEDRIVE` +
/// `HOMEPATH`) instead. Reading `HOME` alone would resolve the session store to
/// a drive root there, so every platform gets the same fallback chain here.
pub fn home_dir() -> PathBuf {
    home_dir_opt().unwrap_or_else(|| PathBuf::from("."))
}

/// [`home_dir`] when the platform gives one. `None` keeps a caller that would
/// *write* there from writing to a relative path instead.
pub fn home_dir_opt() -> Option<PathBuf> {
    if let Some(home) = std::env::var_os("HOME").filter(|value| !value.is_empty()) {
        return Some(PathBuf::from(home));
    }
    if let Some(profile) = std::env::var_os("USERPROFILE").filter(|value| !value.is_empty()) {
        return Some(PathBuf::from(profile));
    }
    match (std::env::var_os("HOMEDRIVE"), std::env::var_os("HOMEPATH")) {
        // `HOMEPATH` is root-relative (`\Users\name`), so `join` appends it to
        // the drive rather than replacing it.
        (Some(drive), Some(path)) if !drive.is_empty() && !path.is_empty() => {
            Some(PathBuf::from(drive).join(path))
        }
        _ => None,
    }
}

/// Platform-correct labels for the primary shortcut modifier.
///
/// Keybindings bind GPUI's `secondary` token (Cmd on macOS, Ctrl elsewhere),
/// so UI hint chips must render the matching chord rather than a hardcoded
/// `⌘`, which on Windows would advertise the Windows/Super key instead.
/// The app's canonical key chords, in the syntax `KeyBinding` parses:
/// `secondary` resolves to Cmd on macOS and Ctrl elsewhere, so one table
/// drives both platforms. Display strings come from [`label`], never from a
/// hand-typed glyph, so a binding and its chip cannot drift apart.
pub mod shortcuts {
    pub const NEW_SESSION: &str = "secondary-n";
    pub const REFRESH: &str = "secondary-r";
    pub const TERMINAL: &str = "secondary-j";
    /// Terminal tabs, scoped to the `Terminal` key context.
    ///
    /// macOS keeps the editor convention: ⌘T opens a tab, ⌘W closes it (not the
    /// window). Elsewhere `secondary` is Ctrl, and Ctrl+T / Ctrl+W belong to the
    /// shell — readline's `transpose-chars` and `unix-word-rubout` — so the
    /// shifted chords carry the tab actions instead.
    #[cfg(target_os = "macos")]
    pub const NEW_TERMINAL_TAB: &str = "secondary-t";
    #[cfg(not(target_os = "macos"))]
    pub const NEW_TERMINAL_TAB: &str = "secondary-shift-t";
    #[cfg(target_os = "macos")]
    pub const CLOSE_TERMINAL_TAB: &str = "secondary-w";
    #[cfg(not(target_os = "macos"))]
    pub const CLOSE_TERMINAL_TAB: &str = "secondary-shift-w";
    /// Step between tabs with the bracket pair, the way macOS's own Terminal
    /// does (Ctrl+Tab belongs to the shell).
    ///
    /// Both backends report shifted punctuation as the shifted symbol with
    /// `shift` cleared, so the chord that matches a real key press is `}` /
    /// `{`; `shift-]` / `shift-[` would never fire. [`label`] renders these
    /// back as ⇧] / ⇧[.
    pub const NEXT_TERMINAL_TAB: &str = "secondary-}";
    pub const PREV_TERMINAL_TAB: &str = "secondary-{";
    pub const SIDEBAR: &str = "secondary-b";
    pub const FOCUS_SESSIONS: &str = "secondary-shift-b";
    /// The session-number span; `⌘1…⌘9` bound by `commands::BINDINGS`.
    pub const FIRST_SESSION_SLOT: &str = "secondary-1";
    pub const LAST_SESSION_SLOT: &str = "secondary-9";
    pub const PROJECT_PANEL: &str = "secondary-shift-e";
    pub const SETTINGS: &str = "secondary-,";
    pub const PALETTE: &str = "secondary-p";
    pub const PALETTE_ALT: &str = "secondary-k";
    pub const FIND: &str = "secondary-f";
    pub const USAGE: &str = "secondary-u";
    pub const PREV_TURN: &str = "secondary-up";
    pub const NEXT_TURN: &str = "secondary-down";
    pub const COPY_LAST_RESPONSE: &str = "secondary-shift-c";
    pub const CHECK_UPDATES: &str = "secondary-shift-u";
    pub const QUIT: &str = "secondary-q";
    pub const CHOOSE_MODEL: &str = "secondary-shift-m";
    pub const CHOOSE_THINKING: &str = "secondary-shift-t";
    pub const REVIEW_CHANGES: &str = "secondary-shift-r";
    pub const OPEN_GIT: &str = "secondary-shift-g";
    pub const SHORTCUT_HELP: &str = "secondary-/";
    pub const SEND: &str = "enter";
    pub const SEND_ALTERNATE: &str = "alt-enter";
    pub const STEER: &str = "secondary-shift-enter";
    pub const NEWLINE: &str = "shift-enter";
    pub const ACCEPT: &str = "tab";
    pub const STOP: &str = "escape";

    /// Render one canonical chord the way this platform labels it: macOS
    /// glyphs (`⌘⇧B`) or explicit names (`Ctrl+Shift+B`).
    ///
    /// Only the chord vocabulary `KeyBinding` accepts is handled; an unknown
    /// key falls back to its raw spelling so a label is never blank.
    pub fn label(chord: &str) -> String {
        #[cfg(target_os = "macos")]
        {
            let mut out = String::new();
            for part in chord.split('-') {
                match part {
                    "secondary" | "cmd" | "super" | "win" => out.push('⌘'),
                    "shift" => out.push('⇧'),
                    "alt" => out.push('⌥'),
                    "ctrl" | "control" => out.push('⌃'),
                    "fn" => out.push_str("fn"),
                    key => out.push_str(&key_label_mac(key)),
                }
            }
            out
        }
        #[cfg(not(target_os = "macos"))]
        {
            let mut parts: Vec<String> = Vec::new();
            for part in chord.split('-') {
                match part {
                    "secondary" | "cmd" | "super" | "win" | "ctrl" | "control" => {
                        parts.push("Ctrl".to_string())
                    }
                    "shift" => parts.push("Shift".to_string()),
                    "alt" => parts.push("Alt".to_string()),
                    "fn" => parts.push("Fn".to_string()),
                    key => parts.push(key_label_other(key)),
                }
            }
            parts.join("+")
        }
    }

    #[cfg(target_os = "macos")]
    fn key_label_mac(key: &str) -> String {
        match key {
            "enter" => "↵".to_string(),
            "escape" => "Esc".to_string(),
            "tab" => "Tab".to_string(),
            "space" => "Space".to_string(),
            "up" => "↑".to_string(),
            "down" => "↓".to_string(),
            "left" => "←".to_string(),
            "right" => "→".to_string(),
            "pageup" => "⇞".to_string(),
            "pagedown" => "⇟".to_string(),
            "home" => "↖".to_string(),
            "end" => "↘".to_string(),
            "backspace" => "⌫".to_string(),
            "delete" => "⌦".to_string(),
            // The backends deliver shifted punctuation as the shifted symbol
            // with `shift` cleared, so the bracket-pair chord is `}` / `{`;
            // render the chord the keyboard actually asks for.
            "{" => "⇧[".to_string(),
            "}" => "⇧]".to_string(),
            other if other.chars().count() == 1 => other.to_uppercase(),
            other => other.to_string(),
        }
    }

    #[cfg(not(target_os = "macos"))]
    fn key_label_other(key: &str) -> String {
        match key {
            "enter" => "Enter".to_string(),
            "escape" => "Esc".to_string(),
            "tab" => "Tab".to_string(),
            "space" => "Space".to_string(),
            "up" => "↑".to_string(),
            "down" => "↓".to_string(),
            "left" => "←".to_string(),
            "right" => "→".to_string(),
            "pageup" => "PgUp".to_string(),
            "pagedown" => "PgDn".to_string(),
            "backspace" => "Backspace".to_string(),
            "delete" => "Delete".to_string(),
            // See [`key_label_mac`]: shifted punctuation arrives as the shifted
            // symbol, so spell the chord the keyboard actually asks for.
            "{" => "Shift+[".to_string(),
            "}" => "Shift+]".to_string(),
            other if other.chars().count() == 1 => other.to_uppercase(),
            other => other.to_string(),
        }
    }
}

/// The catalog id of the platform's own file manager, preferred when the user
/// has not chosen an app. macOS calls it Finder; Windows, File Explorer.
#[cfg(target_os = "macos")]
const DEFAULT_OPEN_IN_FILE_MANAGER: &str = "finder";
#[cfg(not(target_os = "macos"))]
const DEFAULT_OPEN_IN_FILE_MANAGER: &str = "explorer";

fn open_in_prefs_path() -> PathBuf {
    home_dir().join(".orbit-pi").join("open-in.json")
}

/// Workspace-specific app choices, with the old global choice as a fallback.
/// Keys are workspace paths, not session ids, so sessions in a project share
/// the same choice without writing anything into the project itself.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(default)]
pub struct OpenInPrefs {
    open_in_app: Option<String>,
    workspaces: BTreeMap<PathBuf, String>,
}

impl OpenInPrefs {
    pub fn load() -> Self {
        match Self::load_from(&open_in_prefs_path()) {
            Ok(prefs) => prefs,
            Err(error) => {
                eprintln!("Could not load open-in preferences: {error}");
                Self::default()
            }
        }
    }

    fn load_from(path: &Path) -> std::io::Result<Self> {
        match std::fs::read(path) {
            Ok(raw) => serde_json::from_slice(&raw).map_err(std::io::Error::other),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(error) => Err(error),
        }
    }

    /// Resolve against installed apps without discarding an unavailable saved
    /// choice. Reinstalling that app restores it as the preferred target.
    pub fn preferred_app<'a>(
        &self,
        workspace: Option<&Path>,
        apps: &'a [ExternalApp],
    ) -> Option<&'a ExternalApp> {
        let workspace_app = workspace.and_then(|path| self.workspaces.get(path));
        [workspace_app, self.open_in_app.as_ref()]
            .into_iter()
            .flatten()
            .find_map(|id| apps.iter().find(|app| app.id == id))
            .or_else(|| {
                apps.iter()
                    .find(|app| app.id == DEFAULT_OPEN_IN_FILE_MANAGER)
            })
            .or_else(|| apps.first())
    }

    pub fn remember(&mut self, workspace: &Path, app_id: &str) {
        self.workspaces
            .insert(workspace.to_path_buf(), app_id.to_owned());
    }

    /// Persist off-thread. The caller serializes saves so the latest choice
    /// wins even when the user changes it again before a write completes.
    pub fn persist(&self) -> std::io::Result<()> {
        self.persist_to(&open_in_prefs_path())
    }

    fn persist_to(&self, path: &Path) -> std::io::Result<()> {
        let raw = serde_json::to_vec_pretty(self).map_err(std::io::Error::other)?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        // Keep an interrupted write from truncating the saved preferences.
        let temporary = path.with_extension(format!("json.{}.tmp", std::process::id()));
        std::fs::write(&temporary, raw)?;
        std::fs::rename(temporary, path)
    }
}

/// Run an interactive shell command in the user's terminal. Used for
/// `pi /login <provider>`, whose OAuth/device flow is a terminal UI Orbit
/// cannot render. Best-effort: failures leave the caller to surface a hint.
#[cfg(target_os = "macos")]
pub fn open_terminal_command(command: &str) -> Result<(), String> {
    // A `.command` file opened with `open` runs in Terminal.app without
    // requiring Automation (AppleScript) permission, and works even when a
    // non-default terminal is installed.
    let path = crate::temp_files::ensure_dir()
        .map_err(|err| tr!("platform.login_script_failed", error = err))?
        .join(format!("login-{}.command", std::process::id()));
    let script = format!("#!/bin/zsh\n{command}\n");
    std::fs::write(&path, script)
        .map_err(|err| tr!("platform.login_script_failed", error = err))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700));
    }
    std::process::Command::new("/usr/bin/open")
        .arg(&path)
        .spawn()
        .map(|_| ())
        .map_err(|err| tr!("platform.terminal_failed", error = err))
}

#[cfg(not(target_os = "macos"))]
pub fn open_terminal_command(command: &str) -> Result<(), String> {
    // Windows opens a console running the command through the shell; `start`
    // gives it its own window instead of attaching to this process.
    #[cfg(target_os = "windows")]
    {
        let mut launcher = std::process::Command::new("cmd");
        launcher.args(["/C", "start", "cmd", "/K", command]);
        orbit_rpc::hide_console(&mut launcher);
        launcher
            .spawn()
            .map(|_| ())
            .map_err(|err| tr!("platform.terminal_failed", error = err))
    }
    #[cfg(not(target_os = "windows"))]
    {
        for terminal in ["x-terminal-emulator", "gnome-terminal", "konsole", "xterm"] {
            let mut cmd = std::process::Command::new(terminal);
            match terminal {
                "gnome-terminal" => cmd.arg("--").args(["sh", "-c", command]),
                _ => cmd.args(["-e", "sh", "-c", command]),
            };
            if cmd.spawn().is_ok() {
                return Ok(());
            }
        }
        Err(tr!("platform.no_terminal_found"))
    }
}

/// Open an `http(s)` URL in the user's default browser. Used for the OAuth
/// authorization leg of a browser login; the URL comes from pi's `auth.*`
/// events, so the scheme is validated before launching.
#[cfg(target_os = "macos")]
pub fn open_url(url: &str) -> Result<(), String> {
    if !is_safe_browser_url(url) {
        return Err(tr!("platform.unsafe_url"));
    }
    std::process::Command::new("/usr/bin/open")
        .arg(url)
        .spawn()
        .map(|_| ())
        .map_err(|err| tr!("platform.browser_failed", error = err))
}

#[cfg(not(target_os = "macos"))]
pub fn open_url(url: &str) -> Result<(), String> {
    if !is_safe_browser_url(url) {
        return Err(tr!("platform.unsafe_url"));
    }
    let opener = if cfg!(target_os = "windows") {
        "cmd"
    } else {
        "xdg-open"
    };
    let mut command = std::process::Command::new(opener);
    if cfg!(target_os = "windows") {
        command.args(["/C", "start", "", url]);
    } else {
        command.arg(url);
    }
    orbit_rpc::hide_console(&mut command);
    command
        .spawn()
        .map(|_| ())
        .map_err(|err| tr!("platform.browser_failed", error = err))
}

/// Rename the running process so macOS labels the application menu with the
/// product name instead of the executable (`orbit-pi` under `cargo run`).
///
/// AppKit ignores any title set on the first menu item and always paints the
/// resolved application name — `CFBundleName` for a bundle, else the process
/// name — so the title must be changed at the source. The Carbon Process
/// Manager call below is the supported-by-practice way to do that at runtime;
/// it also keeps the app menu bold. (`NSMenu.setTitle` on the app submenu is
/// ignored by current macOS, so it is not used.)
#[cfg(target_os = "macos")]
pub fn set_process_name(name: &str) {
    use std::ffi::{c_char, c_int, c_void, CString};

    #[repr(C)]
    struct ProcessSerialNumber {
        high_long_of_psn: u32,
        low_long_of_psn: u32,
    }
    type GetCurrentProcessFn = unsafe extern "C" fn(*mut ProcessSerialNumber) -> c_int;
    type CpSetProcessNameFn =
        unsafe extern "C" fn(*const ProcessSerialNumber, *const c_char) -> c_int;

    // `RTLD_DEFAULT` (-2): search images AppKit has already loaded.
    const RTLD_DEFAULT: *mut c_void = -2isize as *mut c_void;
    let Ok(name) = CString::new(name) else {
        return;
    };

    // SAFETY: both symbols are resolved by name from ApplicationServices and
    // transmuted to their documented C signatures before use.
    unsafe {
        let get_sym = libc::dlsym(RTLD_DEFAULT, c"GetCurrentProcess".as_ptr());
        let set_sym = libc::dlsym(RTLD_DEFAULT, c"CPSSetProcessName".as_ptr());
        if get_sym.is_null() || set_sym.is_null() {
            return;
        }
        let get: GetCurrentProcessFn = std::mem::transmute(get_sym);
        let set: CpSetProcessNameFn = std::mem::transmute(set_sym);
        let mut psn = ProcessSerialNumber {
            high_long_of_psn: 0,
            low_long_of_psn: 0,
        };
        if get(&mut psn) == 0 {
            let _ = set(&psn, name.as_ptr());
        }
    }
}

/// Menu bars only exist on macOS; GPUI's Windows/Linux platforms store the
/// menu but never paint one, so there is nothing to relabel.
#[cfg(not(target_os = "macos"))]
pub fn set_process_name(_name: &str) {}

/// Only `http`/`https` URLs reach the OS opener, so a compromised or buggy
/// server can never hand the shell a `file:`/`javascript:` target.
fn is_safe_browser_url(url: &str) -> bool {
    let trimmed = url.trim();
    trimmed.starts_with("https://") || trimmed.starts_with("http://")
}

/// Open System Settings → Notifications so the user can unblock Orbit's
/// banners. macOS 13+ (the app's floor) uses the Notifications extension
/// URL; the legacy `com.apple.preference` pane is the older fallback.
#[cfg(target_os = "macos")]
pub fn open_notification_settings() -> Result<(), String> {
    const PANES: [&str; 2] = [
        "x-apple.systempreferences:com.apple.Notifications-Settings.extension",
        "x-apple.systempreferences:com.apple.preference.notifications",
    ];
    for pane in PANES {
        let opened = std::process::Command::new("/usr/bin/open")
            .arg(pane)
            .status()
            .map(|status| status.success())
            .unwrap_or(false);
        if opened {
            return Ok(());
        }
    }
    Err(tr!("platform.open_settings_failed"))
}

#[cfg(not(target_os = "macos"))]
pub fn open_notification_settings() -> Result<(), String> {
    Ok(())
}

// ── host description ────────────────────────────────────────────────────

/// The machine Orbit is running on, as the setup page reports it.
pub struct Host {
    /// Human label: `Windows 11 (build 26100)`, `macOS 15.2`,
    /// `Ubuntu 24.04.1 LTS`.
    pub label: String,
    /// Rust's target architecture, e.g. `x86_64` or `aarch64`.
    pub arch: &'static str,
    /// Set when the host is older than this build supports, naming the floor.
    /// A host whose version can't be read is never reported as unsupported.
    pub unsupported: Option<String>,
}

/// A version read off the host, compared against the floors each bundle
/// declares. Only the fields the host exposes are filled: Windows reports a
/// build number, macOS a major/minor pair.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct OsVersion {
    major: u32,
    minor: u32,
    build: u32,
}

/// Probe the host OS for the setup page.
///
/// A version is only reachable by running something — `sw_vers` on macOS,
/// `ver` on Windows — or by reading `/etc/os-release` on Linux; Orbit takes no
/// dependency for one field of one page. When the probe fails the label falls
/// back to the `std::env::consts::OS` name, which is still right about the
/// family.
pub fn host() -> Host {
    os_probe()
}

/// Run `command args`, returning trimmed stdout when it succeeds.
fn command_output(command: &str, args: &[&str]) -> Option<String> {
    let mut command = std::process::Command::new(command);
    command.args(args);
    orbit_rpc::hide_console(&mut command);
    let output = command.output().ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    (!text.is_empty()).then_some(text)
}

/// The bare OS family name, used when a version probe fails.
fn os_family() -> String {
    match std::env::consts::OS {
        "macos" => "macOS".into(),
        "windows" => "Windows".into(),
        other => other.to_owned(),
    }
}

/// macOS 13 is the floor in `package.metadata.bundle`'s
/// `osx_minimum_system_version` and in the `LSMinimumSystemVersion` both
/// bundles write, so the setup page states the same number the OS enforces.
#[cfg(target_os = "macos")]
fn os_probe() -> Host {
    let (label, version) = match command_output("sw_vers", &["-productVersion"])
        .as_deref()
        .and_then(parse_macos_version)
    {
        Some((label, version)) => (label, Some(version)),
        None => (os_family(), None),
    };
    Host {
        label,
        arch: std::env::consts::ARCH,
        unsupported: version.and_then(unsupported_reason),
    }
}

/// Windows 10 1809 (build 17763) is the floor: GPUI composes through DXGI
/// flip-model presentation, which earlier builds don't expose.
#[cfg(windows)]
fn os_probe() -> Host {
    // `ver` prints `Microsoft Windows [Version 10.0.26100.9168]`. The 10.0
    // major-minor pair covers both Windows 10 and 11, so the build number is
    // what distinguishes them.
    let (label, version) = match command_output("cmd", &["/C", "ver"])
        .as_deref()
        .and_then(parse_windows_version)
    {
        Some((label, version)) => (label, Some(version)),
        None => (os_family(), None),
    };
    Host {
        label,
        arch: std::env::consts::ARCH,
        unsupported: version.and_then(unsupported_reason),
    }
}

/// Whether a host version is below this target's floor, and what it needs.
///
/// One rule per platform, in one place, so the probe stays a probe and the
/// floors are testable without the machine the test runs on.
fn unsupported_reason(version: OsVersion) -> Option<String> {
    #[cfg(target_os = "macos")]
    {
        const FLOOR_MAJOR: u32 = 13;
        if version.major > 0 && version.major < FLOOR_MAJOR {
            return Some(tr!("platform.needs_macos", major = FLOOR_MAJOR));
        }
    }
    #[cfg(windows)]
    {
        const FLOOR_BUILD: u32 = 17763;
        if version.build > 0 && version.build < FLOOR_BUILD {
            return Some(tr!("platform.needs_windows").to_string());
        }
    }
    // Linux has no floor to compare against (see `os_probe`).
    let _ = version;
    None
}

/// Linux kernels don't carry a distribution, so the label comes from
/// `/etc/os-release`. There is no version floor to compare against: the same
/// build runs on any distro with a Vulkan or GL driver.
#[cfg(all(unix, not(target_os = "macos")))]
fn os_probe() -> Host {
    let label = std::fs::read_to_string("/etc/os-release")
        .ok()
        .and_then(|contents| parse_os_release(&contents))
        .unwrap_or_else(os_family);
    Host {
        label,
        arch: std::env::consts::ARCH,
        unsupported: None,
    }
}

/// `15.2.1` → `("macOS 15.2", 15.2)`.
#[cfg(any(target_os = "macos", test))]
fn parse_macos_version(raw: &str) -> Option<(String, OsVersion)> {
    let mut parts = raw.trim().split('.').map(str::trim);
    let major: u32 = parts.next()?.parse().ok()?;
    let minor: u32 = parts.next().unwrap_or("0").parse().unwrap_or(0);
    Some((
        format!("macOS {major}.{minor}"),
        OsVersion {
            major,
            minor,
            build: 0,
        },
    ))
}

/// `Microsoft Windows [Version 10.0.26100.9168]` →
/// `("Windows 11 (build 26100)", 10.0.26100)`. The marketing name follows the
/// build number: 22000 and up is Windows 11, everything the 10.0 line covers
/// below that is Windows 10.
#[cfg(any(windows, test))]
fn parse_windows_version(raw: &str) -> Option<(String, OsVersion)> {
    let inside = raw.split_once("Version ")?.1;
    let inside = inside.trim_end_matches(']').trim();
    let mut parts = inside.split('.').map(str::trim);
    let major: u32 = parts.next()?.parse().ok()?;
    let minor: u32 = parts.next().unwrap_or("0").parse().unwrap_or(0);
    let build: u32 = parts.next().unwrap_or("0").parse().unwrap_or(0);
    let edition = if build >= 22000 {
        "Windows 11"
    } else {
        "Windows 10"
    };
    let label = if build > 0 {
        format!("{edition} (build {build})")
    } else {
        edition.to_string()
    };
    Some((
        label,
        OsVersion {
            major,
            minor,
            build,
        },
    ))
}

/// `PRETTY_NAME="Ubuntu 24.04.1 LTS"` out of `/etc/os-release`.
#[cfg(any(all(unix, not(target_os = "macos")), test))]
fn parse_os_release(contents: &str) -> Option<String> {
    contents.lines().find_map(|line| {
        let (key, value) = line.split_once('=')?;
        (key.trim() == "PRETTY_NAME").then(|| value.trim().trim_matches('"').to_owned())
    })
}

// ── window chrome ───────────────────────────────────────────────────────

/// Clearance the app's own titlebar controls must leave for the OS's window
/// buttons. macOS draws its traffic lights *inside* the transparent titlebar
/// (see [`titlebar_options`]), so the controls start past them; every other
/// platform keeps the system titlebar, whose buttons sit above the client
/// area, and only needs a plain inset from the window edge.
pub const WINDOW_CONTROLS_CLEARANCE: f32 = if cfg!(target_os = "macos") { 80. } else { 12. };

/// What one of the app's caption buttons asks the window to do. These mirror
/// the system commands, so the platform performs the action — the app never
/// reimplements window management.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WindowCommand {
    Minimize,
    /// Maximize when the window is not maximized, restore it when it is —
    /// what the system's own maximize button does.
    ToggleMaximize,
    Close,
}

/// Windows is the one platform where the app paints the caption itself: macOS
/// gets AppKit's traffic lights inside the transparent titlebar, and Linux
/// decorates through the window manager. See [`titlebar_options`].
pub fn draws_window_controls() -> bool {
    cfg!(windows)
}

/// Begin an OS-driven window move, the way grabbing a titlebar does.
///
/// Windows only, and not through GPUI's `WindowControlArea::Drag`: that path
/// needs the press to reach the platform unhandled, and this app's root
/// tracks focus, so GPUI's focus-transfer listener calls
/// `Window::prevent_default` on every press inside the window. A prevented
/// press is treated as handled, which makes GPUI drop the window-control
/// path entirely — no move loop, and no caption-button commands. So the press
/// hands the move to the OS itself (`WM_SYSCOMMAND` with `SC_MOVE | HTCAPTION`),
/// the same command `DefWindowProc` runs for a caption drag. That message is
/// posted rather than sent so the move loop does not nest inside the
/// mouse-down update — see `windows_chrome::start_drag` for why that matters.
#[cfg_attr(not(windows), allow(dead_code))]
pub fn start_window_drag(window: &Window) {
    #[cfg(windows)]
    windows_chrome::start_drag(window);
    #[cfg(not(windows))]
    let _ = window;
}

/// Perform [`WindowCommand`] — the same system command the corresponding
/// caption button sends.
pub fn window_command(window: &Window, command: WindowCommand) {
    #[cfg(windows)]
    windows_chrome::command(window, command);
    #[cfg(not(windows))]
    let _ = (window, command);
}

/// The raw `user32` calls behind [`start_window_drag`] and [`window_command`].
///
/// Declared here rather than pulled in with a windows-sys feature: four
/// functions and six constants is less code than the dependency, and the
/// signatures are documented on MSDN (`ReleaseCapture`, `SendMessageW`,
/// `PostMessageW`).
#[cfg(windows)]
mod windows_chrome {
    use std::ffi::c_void;

    use gpui::Window;
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};

    use super::WindowCommand;

    const WM_SYSCOMMAND: u32 = 0x0112;
    const SC_MOVE: usize = 0xF010;
    const HTCAPTION: usize = 2;
    const SC_MINIMIZE: usize = 0xF020;
    const SC_MAXIMIZE: usize = 0xF030;
    const SC_RESTORE: usize = 0xF120;
    const SC_CLOSE: usize = 0xF060;

    #[link(name = "user32")]
    extern "system" {
        fn ReleaseCapture() -> i32;
        fn PostMessageW(hwnd: *mut c_void, msg: u32, wparam: usize, lparam: isize) -> i32;
    }

    /// The window's `HWND`. `None` if the platform refuses to hand out a
    /// handle, in which case the caller simply does nothing.
    fn hwnd(window: &Window) -> Option<*mut c_void> {
        // `Window::window_handle` is GPUI's own accessor; the raw handle is
        // the trait method, so it has to be named through the trait.
        let handle = HasWindowHandle::window_handle(window).ok()?;
        match handle.as_raw() {
            RawWindowHandle::Win32(handle) => Some(handle.hwnd.get() as *mut c_void),
            _ => None,
        }
    }

    pub(super) fn start_drag(window: &Window) {
        let Some(hwnd) = hwnd(window) else {
            return;
        };
        // SAFETY: `hwnd` is the window's own handle for as long as `window`
        // lives. `ReleaseCapture` drops the current capture so the window can
        // take it, and `WM_SYSCOMMAND` with `SC_MOVE | HTCAPTION` is how
        // `DefWindowProc` starts a caption drag — including the restore of a
        // maximized window — so the system performs the move and the app never
        // reimplements window management.
        //
        // Posted, not sent. `SendMessageW` runs the move loop *synchronously*,
        // i.e. still inside the mouse-down event this handler is called from.
        // GPUI moves a window's state out of its window map for the duration
        // of that event update (`App::update_window_id` takes it), so every
        // `WM_SIZE` the loop delivers re-enters `handle.update` while the
        // window is already taken — the update fails, `bounds_changed` never
        // runs, and `viewport_size` stays stuck at the maximized size. The
        // layout then paints at the old width inside the smaller restored
        // window (issue #15). Posting defers the loop to the message queue, so
        // it runs *after* the update has put the window back and its resize
        // callbacks can land. `PostMessageW` returns immediately; the move
        // loop owns the mouse until the user releases it.
        //
        // `WM_NCLBUTTONDOWN` with `HTCAPTION` deliberately is not used here:
        // GPUI dispatches a synthetic `MouseDown` for that message, and when
        // its coordinates land back in a header drag region the handler posts
        // another one. A window that covers the screen origin — every window
        // maximized on the primary display — never drains that queue and the
        // app spins forever (issue #26). `WM_SYSCOMMAND` starts the same move
        // loop without re-entering the app's input dispatch.
        unsafe {
            ReleaseCapture();
            PostMessageW(hwnd, WM_SYSCOMMAND, SC_MOVE | HTCAPTION, 0);
        }
    }

    pub(super) fn command(window: &Window, command: WindowCommand) {
        let Some(hwnd) = hwnd(window) else {
            return;
        };
        let sc = match command {
            WindowCommand::Minimize => SC_MINIMIZE,
            WindowCommand::Close => SC_CLOSE,
            WindowCommand::ToggleMaximize => {
                if window.is_maximized() {
                    SC_RESTORE
                } else {
                    SC_MAXIMIZE
                }
            }
        };
        // SAFETY: as above — the handle belongs to this window, and
        // `WM_SYSCOMMAND` is how a window asks the system to run one of its
        // own commands. Posted rather than sent so the command is handled
        // after the click returns, like a real caption button.
        unsafe {
            PostMessageW(hwnd, WM_SYSCOMMAND, sc, 0);
        }
    }
}

/// Width the app's caption buttons occupy at the right of the header row —
/// three 46px buttons, the metric Windows uses for its own. Headers that reach
/// the window's right edge keep their content clear of it.
pub const WINDOW_CONTROLS_W: f32 = 46. * 3.;

/// The window's titlebar configuration.
///
/// macOS gets the transparent titlebar the app draws its own controls into,
/// with the traffic lights moved onto the sidebar's 44px row; Windows gets one
/// too, because it paints its own caption buttons in that same row
/// (`draws_window_controls`) and a system caption above them would
/// double the header. That trade is deliberate: the app's buttons carry
/// `WindowControlArea` hit areas, so minimize/maximize/close, `Alt+Space`, and
/// double-click-to-maximize all still run the system's own commands, but the
/// Win11 snap-layouts flyout — which only a real system caption button opens —
/// is not available. Linux ignores [`TitlebarOptions`] (it decorates through
/// `WindowOptions::window_decorations`), so neither branch has to be right for
/// it.
#[cfg(target_os = "macos")]
pub fn titlebar_options() -> TitlebarOptions {
    TitlebarOptions {
        title: Some(SharedString::from("Orbit Pi")),
        // Transparent titlebar: the sidebar extends to the top and the native
        // traffic lights sit inside it.
        appears_transparent: true,
        // Center the lights in the 44px titlebar row so they share a line with
        // the window controls beside them.
        traffic_light_position: Some(point(px(12.), px(16.))),
    }
}

/// Transparent on Windows for the same reason as macOS — the app draws the
/// caption buttons itself — and `false` anywhere else, where nothing would.
#[cfg(not(target_os = "macos"))]
pub fn titlebar_options() -> TitlebarOptions {
    TitlebarOptions {
        title: Some(SharedString::from("Orbit Pi")),
        appears_transparent: draws_window_controls(),
        traffic_light_position: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chord_labels_use_the_platform_vocabulary() {
        assert_eq!(
            shortcuts::label("secondary-n"),
            if cfg!(target_os = "macos") {
                "⌘N"
            } else {
                "Ctrl+N"
            }
        );
        assert_eq!(shortcuts::label("escape"), "Esc");
        assert_eq!(shortcuts::label("tab"), "Tab");
        assert_eq!(
            shortcuts::label("secondary-up"),
            if cfg!(target_os = "macos") {
                "⌘↑"
            } else {
                "Ctrl+↑"
            }
        );
        assert_eq!(
            shortcuts::label("secondary-shift-enter"),
            if cfg!(target_os = "macos") {
                "⌘⇧↵"
            } else {
                "Ctrl+Shift+Enter"
            }
        );
        // The tab-cycle chord is the shifted symbol the backends deliver, but
        // the chip still advertises the shift chord it takes to type it.
        assert_eq!(
            shortcuts::label(shortcuts::NEXT_TERMINAL_TAB),
            if cfg!(target_os = "macos") {
                "⌘⇧]"
            } else {
                "Ctrl+Shift+]"
            }
        );
        assert_eq!(
            shortcuts::label(shortcuts::PREV_TERMINAL_TAB),
            if cfg!(target_os = "macos") {
                "⌘⇧["
            } else {
                "Ctrl+Shift+["
            }
        );
    }

    fn open_in_test_apps() -> Vec<ExternalApp> {
        ["vscode", "rider", DEFAULT_OPEN_IN_FILE_MANAGER]
            .into_iter()
            .map(|id| ExternalApp {
                id,
                label: id,
                target: id.to_string(),
                icon: Arc::new(Image::from_bytes(gpui::ImageFormat::Png, Vec::new())),
            })
            .collect()
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn open_in_catalog_includes_rider_stable_and_eap() {
        let rider = OPEN_IN_CATALOG
            .iter()
            .find(|entry| entry.0 == "rider")
            .unwrap();
        assert_eq!(rider.1, "Rider");
        assert_eq!(rider.2, &["com.jetbrains.rider", "com.jetbrains.rider-EAP"]);
    }

    /// The icon the menu embeds must be exactly the pixel size the 18px slot
    /// blits 1:1 on a Retina window — not whatever representation AppKit
    /// happens to hold.
    #[cfg(target_os = "macos")]
    #[test]
    fn open_in_app_icon_is_rasterized_at_the_menu_pixel_size() {
        use objc2_foundation::NSString;

        let path = NSString::from_str("/System/Library/CoreServices/Finder.app");
        let png = app_icon_png(&path, APP_ICON_PX).expect("Finder always yields an icon");
        let icon = image::load_from_memory(&png).expect("the rasterizer emits a decodable PNG");
        assert_eq!(icon.width(), APP_ICON_PX as u32);
        assert_eq!(icon.height(), APP_ICON_PX as u32);
    }

    #[test]
    fn open_in_legacy_global_choice_is_preserved_as_fallback() {
        let prefs: OpenInPrefs = serde_json::from_str(r#"{"open_in_app":"vscode"}"#).unwrap();
        let apps = open_in_test_apps();
        assert_eq!(
            prefs
                .preferred_app(Some(Path::new("/new-project")), &apps)
                .unwrap()
                .id,
            "vscode"
        );
        assert_eq!(prefs.preferred_app(None, &apps).unwrap().id, "vscode");
    }

    #[test]
    fn open_in_workspace_choices_are_independent_and_survive_reload() {
        let mut prefs: OpenInPrefs = serde_json::from_str(r#"{"open_in_app":"vscode"}"#).unwrap();
        let project_a = Path::new("/projects/Game with spaces");
        let project_b = Path::new("/projects/游戏");
        prefs.remember(project_a, "rider");
        prefs.remember(project_b, DEFAULT_OPEN_IN_FILE_MANAGER);
        let raw = serde_json::to_string(&prefs).unwrap();
        let mut prefs: OpenInPrefs = serde_json::from_str(&raw).unwrap();
        let apps = open_in_test_apps();
        for (path, expected) in [
            (project_a, "rider"),
            (project_b, DEFAULT_OPEN_IN_FILE_MANAGER),
            (project_a, "rider"),
        ] {
            assert_eq!(prefs.preferred_app(Some(path), &apps).unwrap().id, expected);
        }
        prefs.remember(project_a, "vscode");
        assert_eq!(
            prefs.preferred_app(Some(project_a), &apps).unwrap().id,
            "vscode"
        );
        assert_eq!(
            prefs.preferred_app(Some(project_b), &apps).unwrap().id,
            DEFAULT_OPEN_IN_FILE_MANAGER
        );
        assert_eq!(prefs.preferred_app(None, &apps).unwrap().id, "vscode");
    }

    #[test]
    fn open_in_unavailable_workspace_app_falls_back_without_forgetting_choice() {
        let mut prefs: OpenInPrefs = serde_json::from_str(r#"{"open_in_app":"vscode"}"#).unwrap();
        let workspace = Path::new("/project");
        prefs.remember(workspace, "rider");
        let apps = open_in_test_apps();
        let without_rider: Vec<_> = apps
            .iter()
            .filter(|app| app.id != "rider")
            .cloned()
            .collect();
        assert_eq!(
            prefs
                .preferred_app(Some(workspace), &without_rider)
                .unwrap()
                .id,
            "vscode"
        );
        assert_eq!(
            prefs.preferred_app(Some(workspace), &apps).unwrap().id,
            "rider"
        );
    }

    #[test]
    fn open_in_defaults_to_file_manager_then_first_installed_app() {
        let prefs: OpenInPrefs = serde_json::from_str(r#"{"open_in_app":"uninstalled"}"#).unwrap();
        let apps = open_in_test_apps();
        assert_eq!(
            prefs.preferred_app(None, &apps).unwrap().id,
            DEFAULT_OPEN_IN_FILE_MANAGER
        );
        assert_eq!(prefs.preferred_app(None, &apps[..1]).unwrap().id, "vscode");
        assert!(prefs.preferred_app(None, &[]).is_none());
        assert!(OpenInPrefs::default().preferred_app(None, &[]).is_none());
    }

    /// End-to-end smoke test for the Windows icon path: the file manager is
    /// always installed, and its executable must decode to a real PNG.
    #[cfg(windows)]
    #[test]
    fn windows_extracts_a_png_icon_for_explorer() {
        let explorer = std::env::var("WINDIR")
            .map(PathBuf::from)
            .expect("WINDIR is set on Windows")
            .join("explorer.exe");
        let png = windows_open_in::extract_icon_png(&explorer).expect("explorer has an icon");
        assert_eq!(&png[..4], b"\x89PNG");
        assert!(png.len() > 64);
    }

    /// Detection always offers File Explorer on Windows — it ships with the OS.
    #[cfg(windows)]
    #[test]
    fn windows_always_detects_file_explorer() {
        let apps = detect_open_in_apps();
        let explorer = apps
            .iter()
            .find(|app| app.id == "explorer")
            .expect("File Explorer is always present");
        assert!(!explorer.target.is_empty());
        assert!(Path::new(&explorer.target).is_file());
    }

    /// A `%VAR%` path with an unknown variable cannot resolve, and one with a
    /// present variable resolves to that variable's value.
    #[cfg(windows)]
    #[test]
    fn windows_expands_environment_paths() {
        assert!(windows_open_in::expand_env("%ORBIT_DEFINITELY_UNSET_VAR%\\x").is_none());
        let windir = std::env::var("WINDIR").unwrap();
        assert_eq!(
            windows_open_in::expand_env("%WINDIR%\\explorer.exe").unwrap(),
            PathBuf::from(windir).join("explorer.exe")
        );
    }

    #[test]
    fn open_in_persistence_creates_and_replaces_preferences() {
        let dir = std::env::temp_dir().join(format!(
            "orbit-open-in-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let path = dir.join("open-in.json");
        let mut prefs = OpenInPrefs::load_from(&path).unwrap();
        assert!(prefs.workspaces.is_empty());
        prefs.remember(Path::new("/project"), "rider");
        prefs.persist_to(&path).unwrap();
        prefs.remember(Path::new("/project"), "vscode");
        prefs.persist_to(&path).unwrap();
        let reloaded = OpenInPrefs::load_from(&path).unwrap();
        assert_eq!(
            reloaded.workspaces.get(Path::new("/project")).unwrap(),
            "vscode"
        );
        std::fs::write(&path, "not JSON").unwrap();
        assert!(OpenInPrefs::load_from(&path).is_err());
        assert!(prefs.persist_to(&path.join("invalid-child")).is_err());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn only_http_urls_are_opened() {
        assert!(is_safe_browser_url("https://claude.ai/oauth"));
        assert!(is_safe_browser_url("http://127.0.0.1:1455/callback"));
        assert!(!is_safe_browser_url("file:///etc/passwd"));
        assert!(!is_safe_browser_url("javascript:alert(1)"));
        assert!(!is_safe_browser_url("  "));
    }

    /// The Windows label distinguishes 10 from 11 off the build number, since
    /// both report 10.0 as their version.
    #[test]
    #[cfg(any(windows, test))]
    fn windows_label_follows_the_build_number() {
        let eleven = parse_windows_version("Microsoft Windows [Version 10.0.26100.9168]");
        assert_eq!(
            eleven.map(|(label, _)| label).as_deref(),
            Some("Windows 11 (build 26100)")
        );
        let ten = parse_windows_version("Microsoft Windows [Version 10.0.17763.1]");
        assert_eq!(
            ten.map(|(label, _)| label).as_deref(),
            Some("Windows 10 (build 17763)")
        );
        assert!(parse_windows_version("not a version").is_none());
    }

    #[test]
    #[cfg(any(target_os = "macos", test))]
    fn macos_label_keeps_major_and_minor() {
        let parsed = parse_macos_version("15.2.1\n");
        assert_eq!(
            parsed.map(|(label, _)| label).as_deref(),
            Some("macOS 15.2")
        );
        assert!(parse_macos_version("Sequoia").is_none());
    }

    #[test]
    #[cfg(any(all(unix, not(target_os = "macos")), test))]
    fn os_release_prefers_pretty_name() {
        let contents =
            "NAME=\"Ubuntu\"\nVERSION_ID=\"24.04\"\nPRETTY_NAME=\"Ubuntu 24.04.1 LTS\"\n";
        assert_eq!(
            parse_os_release(contents).as_deref(),
            Some("Ubuntu 24.04.1 LTS")
        );
        assert!(parse_os_release("ID=alpine\n").is_none());
    }

    /// The floors are what the bundles declare, so a host below one is called
    /// out rather than left looking supported.
    #[cfg(target_os = "macos")]
    #[test]
    fn macos_below_the_floor_is_unsupported() {
        let old = OsVersion {
            major: 12,
            minor: 7,
            build: 0,
        };
        assert!(unsupported_reason(old).is_some());
        let floor = OsVersion {
            major: 13,
            minor: 0,
            build: 0,
        };
        assert!(unsupported_reason(floor).is_none());
    }

    #[cfg(windows)]
    #[test]
    fn windows_below_the_floor_is_unsupported() {
        let old = OsVersion {
            major: 10,
            minor: 0,
            build: 17134,
        };
        assert!(unsupported_reason(old).is_some());
        let floor = OsVersion {
            major: 10,
            minor: 0,
            build: 17763,
        };
        assert!(unsupported_reason(floor).is_none());
    }

    /// The probe answers with a real label on the machine running the tests.
    #[test]
    fn host_label_is_never_empty() {
        let host = host();
        assert!(!host.label.trim().is_empty());
        assert!(!host.arch.trim().is_empty());
    }
}
