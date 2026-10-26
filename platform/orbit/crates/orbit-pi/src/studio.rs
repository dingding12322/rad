//! OpenViking Studio window.
//!
//! OpenViking ships a web console ("Studio") next to its local server, so the
//! workbench opens it instead of re-implementing a resource browser. On macOS
//! the console runs in a native WKWebView window sharing GPUI's application;
//! elsewhere (and whenever the native window fails) the caller falls back to
//! the system browser. The OpenViking API key from `~/.openviking/ovcli.conf`
//! is injected into `localStorage` so the console authenticates silently.

use crate::platform;

/// Local OpenViking server origin — the same default the OpenCode plugin and
/// the MCP proxy resolve when no override is configured.
const OV_ORIGIN: &str = "http://127.0.0.1:1933";

/// The Studio console SPA.
pub const STUDIO_URL: &str = "http://127.0.0.1:1933/studio/";

// WKWebView's classes live in WebKit; nothing else in the graph links it, so
// `objc_getClass` would come up empty without this.
#[cfg(target_os = "macos")]
#[link(name = "WebKit", kind = "framework")]
extern "C" {}

/// The `localStorage` key holding the Studio console's connection JSON
/// (`accountId`/`adminApiKey`/`apiKey`/`baseUrl`/`userId`).
const CONNECTION_STORE: &str = "ov_console_connection";

/// Whether the local OpenViking server answers `/health`. Blocking network
/// I/O — run on a background executor, never on the UI thread.
pub fn health_ok() -> bool {
    reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(2))
        .build()
        .ok()
        .and_then(|client| {
            client
                .get(format!("{OV_ORIGIN}/health"))
                .send()
                .ok()
                .map(|response| response.status().is_success())
        })
        .unwrap_or(false)
}

/// One top-level string value from `~/.openviking/ovcli.conf` (a JSON
/// plist), or `None` when absent/empty.
fn conf_value(name: &str) -> Option<String> {
    let path = platform::home_dir_opt()?.join(".openviking").join("ovcli.conf");
    let raw = std::fs::read_to_string(path).ok()?;
    let value: serde_json::Value = serde_json::from_str(&raw).ok()?;
    value
        .get(name)
        .and_then(serde_json::Value::as_str)
        .filter(|v| !v.is_empty())
        .map(str::to_owned)
}

/// The OpenViking API key from `~/.openviking/ovcli.conf` (a JSON plist —
/// the same file the OpenCode plugin resolves credentials from), or `None`
/// when absent; the console then asks for the key itself.
fn api_key() -> Option<String> {
    conf_value("api_key")
}

/// Open the Studio console. macOS raises a native WKWebView window; other
/// platforms return `Err` so the caller opens the system browser instead.
pub fn open_window() -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        open_webview_window(STUDIO_URL)
    }
    #[cfg(not(target_os = "macos"))]
    {
        Err(tr!("studio.native_window_unsupported"))
    }
}

/// Open a native WKWebView window on GPUI's shared NSApplication.
///
/// Memory discipline: plain retain/release, no `Id` wrapper. Every temporary
/// `alloc`ed object is released once its receiver owns it; the window is
/// marked `releasedWhenClosed: NO` and intentionally leaked so it stays valid
/// for the process lifetime (opening the console is a rare, user-driven act).
#[cfg(target_os = "macos")]
fn open_webview_window(url: &str) -> Result<(), String> {
    use objc2::{class, msg_send};
    use objc2::runtime::AnyObject;
    use objc2_foundation::{NSPoint, NSRect, NSSize};

    // Titled | closable | miniaturizable | resizable.
    const STYLE_MASK: usize = 1 | 2 | 4 | 8;
    // NSBackingStoreBuffered.
    const BACKING_BUFFERED: isize = 2;
    // NSViewWidthSizable | NSViewHeightSizable.
    const SIZE_TO_FIT_PARENT: usize = 2 | 16;
    // WKUserScriptInjectionTimeAtDocumentStart.
    const INJECT_AT_DOCUMENT_START: isize = 0;
    // Window and webview frames, prebuilt: objc2's msg_send splits arguments
    // on top-level commas, so keep struct literals out of the invocation.
    let window_frame = NSRect {
        origin: NSPoint { x: 120.0, y: 120.0 },
        size: NSSize { width: 1200.0, height: 820.0 },
    };
    let webview_frame = NSRect {
        origin: NSPoint { x: 0.0, y: 0.0 },
        size: NSSize { width: 1200.0, height: 820.0 },
    };

    unsafe {
        let release = |obj: *mut AnyObject| {
            let _: () = msg_send![obj, release];
        };
        // A real NSString from UTF-8: objc2-foundation 0.3 models NSString as
        // `Rc<str>`, which is awkward over raw msg_send, so bridge directly.
        // `stringWithUTF8String:` autoreleases; retain first so the object
        // survives this call stack and every path below releases explicitly.
        let ns_string = |text: &str| -> Result<*mut AnyObject, String> {
            let bytes = std::ffi::CString::new(text).map_err(|_| tr!("studio.invalid_utf8"))?;
            let string: *mut AnyObject =
                msg_send![class!(NSString), stringWithUTF8String: bytes.as_ptr()];
            if string.is_null() {
                return Err(tr!("studio.window_failed"));
            }
            let _: () = msg_send![string, retain];
            Ok(string)
        };

        let window: *mut AnyObject = msg_send![class!(NSWindow), alloc];
        let window: *mut AnyObject = msg_send![window,
            initWithContentRect: window_frame,
            styleMask: STYLE_MASK,
            backing: BACKING_BUFFERED,
            defer: objc2::runtime::Bool::NO,
        ];
        if window.is_null() {
            return Err(tr!("studio.window_failed"));
        }
        // The window outlives this call regardless of close buttons; its
        // reference is never released (see the module comment).
        let _: () = msg_send![window, setReleasedWhenClosed: objc2::runtime::Bool::NO];
        let title = ns_string("OpenViking Studio")?;
        let _: () = msg_send![window, setTitle: title];
        release(title);

        let configuration: *mut AnyObject = msg_send![class!(WKWebViewConfiguration), alloc];
        let configuration: *mut AnyObject = msg_send![configuration, init];
        if configuration.is_null() {
            release(window);
            return Err(tr!("studio.window_failed"));
        }
        if let Some(key) = api_key() {
            // The console reads its connection from the `ov_console_connection`
            // JSON (accountId/adminApiKey/apiKey/baseUrl/userId); writing it
            // at document start skips its manual key prompt. Only fill it in
            // when no key is configured yet, so manual edits win. The script
            // body is data (a key), so it must not close the string it lands in.
            let escaped = key.replace('\\', "\\\\").replace('\'', "\\'");
            let account = conf_value("account").unwrap_or_else(|| "default".to_string());
            let user = conf_value("user").unwrap_or_else(|| "default".to_string());
            let script_body = format!(
                "try{{var k='{CONNECTION_STORE}';var c={{}};try{{c=JSON.parse(localStorage.getItem(k)||'{{}}')}}catch(e){{}};\
                 if(!c.adminApiKey&&!c.apiKey){{localStorage.setItem(k,JSON.stringify({{accountId:'{account}',\
                 adminApiKey:'{escaped}',apiKey:'{escaped}',baseUrl:'{OV_ORIGIN}',userId:'{user}'}}))}}}}catch(e){{}}"
            );
            let source = ns_string(&script_body)?;
            let user_script: *mut AnyObject = msg_send![class!(WKUserScript), alloc];
            let user_script: *mut AnyObject = msg_send![user_script,
                initWithSource: source,
                injectionTime: INJECT_AT_DOCUMENT_START,
                forMainFrameOnly: objc2::runtime::Bool::YES,
            ];
            release(source);
            if !user_script.is_null() {
                // `addUserScript:` lives on WKUserContentController, not on
                // the configuration; the controller is attached via
                // `setUserContentController:`.
                let controller: *mut AnyObject =
                    msg_send![class!(WKUserContentController), alloc];
                let controller: *mut AnyObject = msg_send![controller, init];
                if !controller.is_null() {
                    let _: () = msg_send![controller, addUserScript: user_script];
                    let _: () = msg_send![configuration, setUserContentController: controller];
                    release(controller);
                }
                release(user_script);
            }
        }

        let content_view: *mut AnyObject = msg_send![window, contentView];
        let webview: *mut AnyObject = msg_send![class!(WKWebView), alloc];
        let webview: *mut AnyObject = msg_send![webview,
            initWithFrame: webview_frame,
            configuration: configuration,
        ];
        release(configuration);
        if webview.is_null() || content_view.is_null() {
            if !webview.is_null() {
                release(webview);
            }
            release(window);
            return Err(tr!("studio.window_failed"));
        }
        let _: () = msg_send![content_view, addSubview: webview];
        let _: () = msg_send![webview, setAutoresizingMask: SIZE_TO_FIT_PARENT];

        let url_string = ns_string(url)?;
        let url: *mut AnyObject = msg_send![class!(NSURL), URLWithString: url_string];
        release(url_string);
        if url.is_null() {
            release(webview);
            release(window);
            return Err(tr!("studio.window_failed"));
        }
        let request: *mut AnyObject = msg_send![class!(NSURLRequest), requestWithURL: url];
        release(url);
        let _: () = msg_send![webview, loadRequest: request];
        release(request);

        let _: () = msg_send![window, makeKeyAndOrderFront: std::ptr::null_mut::<AnyObject>()];
        // `content_view` and `webview` are owned by their owners (the window
        // and its hierarchy); no extra references were taken.
        Ok(())
    }
}
