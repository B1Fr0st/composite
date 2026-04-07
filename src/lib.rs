use std::num::NonZeroIsize;
use raw_window_handle::{
    HasWindowHandle, RawWindowHandle, Win32WindowHandle, WindowHandle, HandleError,
};
use winapi::{
    shared::windef::{HWND, RECT},
    um::winuser::{FindWindowA, GetClientRect},
};
use wry::WebViewBuilder;

pub use wry;

/// Messages received from the webview via `window.ipc.postMessage(...)`.
pub type IpcHandler = Box<dyn Fn(String) + Send + 'static>;

/// Configuration for creating an [`Overlay`].
pub struct OverlayConfig {
    /// Whether the webview background should be transparent.
    pub transparent: bool,
    /// Optional handler called when JavaScript sends a message via `window.ipc.postMessage`.
    pub ipc_handler: Option<IpcHandler>,
}

impl Default for OverlayConfig {
    fn default() -> Self {
        Self {
            transparent: true,
            ipc_handler: None,
        }
    }
}

/// Thin wrapper that implements [`HasWindowHandle`] for a raw Win32 HWND,
/// letting wry embed a WebView directly into the Discord window.
struct HwndWrapper(HWND);

impl HasWindowHandle for HwndWrapper {
    fn window_handle(&self) -> Result<WindowHandle<'_>, HandleError> {
        let handle = Win32WindowHandle::new(
            NonZeroIsize::new(self.0 as isize)
                .expect("HWND must be non-null"),
        );
        Ok(unsafe { WindowHandle::borrow_raw(RawWindowHandle::Win32(handle)) })
    }
}

/// A wry [`WebView`](wry::WebView) embedded directly inside the Discord Overlay window.
///
/// No intermediary window is created — the webview is a child of Discord's own HWND.
///
/// # Example
/// ```no_run
/// use newoverlay::{Overlay, OverlayConfig};
///
/// let mut overlay = Overlay::new(OverlayConfig::default()).unwrap();
/// overlay.load_html(r#"<body style="background:transparent"><h1 style="color:red">Hello</h1></body>"#);
/// overlay.run();
/// ```
pub struct Overlay {
    webview: wry::WebView,
    discord_hwnd: HWND,
}

impl Overlay {
    /// Find the Discord Overlay window and embed a WebView into it directly.
    pub fn new(config: OverlayConfig) -> Result<Self, Box<dyn std::error::Error>> {
        let discord_hwnd = unsafe {
            FindWindowA(
                b"Chrome_WidgetWin_1\0".as_ptr() as *const i8,
                b"Discord Overlay\0".as_ptr() as *const i8,
            )
        };

        if discord_hwnd.is_null() {
            return Err("Failed to find Discord Overlay window (is Discord running?)".into());
        }

        println!("Found Discord Overlay window: {:?}", discord_hwnd);

        let mut builder = WebViewBuilder::new()
            .with_transparent(config.transparent);

        if let Some(handler) = config.ipc_handler {
            builder = builder.with_ipc_handler(move |msg| {
                handler(msg.body().to_string());
            });
        }

        let webview = builder.build_as_child(&HwndWrapper(discord_hwnd))?;

        Ok(Self { webview, discord_hwnd })
    }

    /// Load a URL into the webview.
    pub fn load_url(&self, url: &str) {
        let _ = self.webview.load_url(url);
    }

    /// Load a raw HTML string into the webview.
    pub fn load_html(&self, html: &str) {
        let _ = self.webview.load_html(html);
    }

    /// Evaluate a JavaScript expression in the webview.
    pub fn eval(&self, js: &str) {
        let _ = self.webview.evaluate_script(js);
    }

    /// Resize the embedded webview to match the current Discord client rect.
    ///
    /// Call this whenever you detect Discord has been resized.
    pub fn sync_size(&self) {
        if let Some((w, h)) = self.discord_client_size() {
            let _ = self.webview.set_bounds(wry::Rect {
                position: wry::dpi::LogicalPosition::new(0.0, 0.0).into(),
                size: wry::dpi::LogicalSize::new(w as f64, h as f64).into(),
            });
        }
    }

    /// Return a reference to the underlying [`wry::WebView`].
    pub fn webview(&self) -> &wry::WebView {
        &self.webview
    }

    /// Run a simple message-pump loop, keeping the webview sized to Discord and
    /// exiting when Discord's window disappears.
    pub fn run(self) {
        self.sync_size();

        loop {
            // Pump Windows messages so the embedded WebView2 can process events.
            unsafe {
                use winapi::um::winuser::{PeekMessageA, TranslateMessage, DispatchMessageA, PM_REMOVE};
                use winapi::um::winuser::MSG;
                use std::mem;
                use std::ptr;

                let mut msg: MSG = mem::zeroed();
                while PeekMessageA(&mut msg, ptr::null_mut(), 0, 0, PM_REMOVE) != 0 {
                    TranslateMessage(&msg);
                    DispatchMessageA(&msg);

                    if msg.message == winapi::um::winuser::WM_QUIT {
                        return;
                    }
                }
            }

            // Keep webview bounds in sync with Discord
            if self.discord_client_size().is_none() {
                eprintln!("Discord Overlay window lost, shutting down.");
                return;
            }

            self.sync_size();
        }
    }

    fn discord_client_size(&self) -> Option<(u32, u32)> {
        unsafe {
            let mut rect: RECT = std::mem::zeroed();
            if GetClientRect(self.discord_hwnd, &mut rect) == 0 {
                return None;
            }
            let w = (rect.right - rect.left) as u32;
            let h = (rect.bottom - rect.top) as u32;
            if w == 0 || h == 0 { None } else { Some((w, h)) }
        }
    }
}
