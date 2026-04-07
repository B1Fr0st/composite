use std::num::NonZeroIsize;
use std::sync::{Arc, Mutex};
use raw_window_handle::{
    HasWindowHandle, RawWindowHandle, Win32WindowHandle, WindowHandle, HandleError,
};
use winapi::{
    shared::windef::{HWND, RECT, POINT},
    um::winuser::{
        FindWindowA, GetClientRect,
        PeekMessageA, TranslateMessage, DispatchMessageA,
        GetCursorPos, ScreenToClient,
        GetAsyncKeyState,
        MSG, PM_REMOVE,
        VK_LBUTTON, VK_RBUTTON, VK_MBUTTON, VK_XBUTTON1, VK_XBUTTON2,
    },
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
    /// For use with [`Overlay::run`]. If you intend to use [`Overlay::run_with_ipc`], leave
    /// this as `None` — that method installs its own queue-based handler.
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

/// Returns the physical pixel size of the Discord window's client area.
fn discord_client_size(hwnd: HWND) -> Option<(u32, u32)> {
    unsafe {
        let mut rect: RECT = std::mem::zeroed();
        if GetClientRect(hwnd, &mut rect) == 0 {
            return None;
        }
        let w = (rect.right - rect.left) as u32;
        let h = (rect.bottom - rect.top) as u32;
        if w == 0 || h == 0 { None } else { Some((w, h)) }
    }
}

/// A wry [`WebView`](wry::WebView) embedded directly inside the Discord Overlay window.
///
/// No intermediary window is created — the webview is a Win32 child of Discord's own HWND.
pub struct Overlay {
    webview: wry::WebView,
    discord_hwnd: HWND,
    /// Filled when [`Overlay::run_with_ipc`] is used; drained each frame.
    ipc_queue: Option<Arc<Mutex<Vec<String>>>>,
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

        let (w, h) = discord_client_size(discord_hwnd)
            .ok_or("Failed to read Discord client rect")?;

        let mut builder = WebViewBuilder::new()
            .with_transparent(config.transparent)
            .with_bounds(wry::Rect {
                position: wry::dpi::PhysicalPosition::new(0, 0).into(),
                size: wry::dpi::PhysicalSize::new(w, h).into(),
            });

        if let Some(handler) = config.ipc_handler {
            builder = builder.with_ipc_handler(move |msg| {
                handler(msg.body().to_string());
            });
        }

        let webview = builder.build_as_child(&HwndWrapper(discord_hwnd))?;

        Ok(Self { webview, discord_hwnd, ipc_queue: None })
    }

    /// Like [`Overlay::new`], but installs a queue-based IPC handler so that
    /// [`Overlay::run_with_ipc`] can deliver messages to your closure each frame.
    ///
    /// Do not also set `config.ipc_handler` — this replaces it.
    pub fn new_with_ipc_queue(config: OverlayConfig) -> Result<Self, Box<dyn std::error::Error>> {
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

        let (w, h) = discord_client_size(discord_hwnd)
            .ok_or("Failed to read Discord client rect")?;

        let queue: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        let queue_clone = Arc::clone(&queue);

        let builder = WebViewBuilder::new()
            .with_transparent(config.transparent)
            .with_bounds(wry::Rect {
                position: wry::dpi::PhysicalPosition::new(0, 0).into(),
                size: wry::dpi::PhysicalSize::new(w, h).into(),
            })
            .with_ipc_handler(move |msg| {
                if let Ok(mut q) = queue_clone.lock() {
                    q.push(msg.body().to_string());
                }
            });

        let webview = builder.build_as_child(&HwndWrapper(discord_hwnd))?;

        Ok(Self { webview, discord_hwnd, ipc_queue: Some(queue) })
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

    /// Return a reference to the underlying [`wry::WebView`].
    pub fn webview(&self) -> &wry::WebView {
        &self.webview
    }

    /// Resize the webview to exactly fill Discord's current client area (physical pixels).
    pub fn sync_size(&self) {
        if let Some((w, h)) = discord_client_size(self.discord_hwnd) {
            let _ = self.webview.set_bounds(wry::Rect {
                position: wry::dpi::PhysicalPosition::new(0, 0).into(),
                size: wry::dpi::PhysicalSize::new(w, h).into(),
            });
        }
    }

    /// Run the overlay loop, calling `on_ipc` with every IPC message received from JS.
    /// The closure also receives the webview and the current [`wry::Rect`] bounds so it
    /// can reposition/resize the webview in response (e.g. for a draggable panel).
    ///
    /// Requires construction via [`Overlay::new_with_ipc_queue`].
    pub fn run_with_ipc<F>(self, mut on_ipc: F)
    where
        F: FnMut(String, &wry::WebView, wry::Rect),
    {
        let queue = self.ipc_queue.clone()
            .expect("run_with_ipc requires Overlay::new_with_ipc_queue");

        let init_size = discord_client_size(self.discord_hwnd).unwrap_or((720, 480));
        let current_bounds = wry::Rect {
            position: wry::dpi::PhysicalPosition::new(0, 0).into(),
            size: wry::dpi::PhysicalSize::new(init_size.0, init_size.1).into(),
        };

        self.run_loop(false, |webview, _discord_hwnd| {
            let msgs: Vec<String> = queue.lock()
                .map(|mut q| q.drain(..).collect())
                .unwrap_or_default();
            for msg in msgs {
                on_ipc(msg, webview, current_bounds);
            }
        });
    }

    /// Run the overlay loop, calling `on_ipc` with every IPC message.
    /// The closure receives `(msg, webview, &mut current_bounds)` — mutating
    /// `current_bounds` and calling `webview.set_bounds` keeps everything in sync.
    pub fn run_with_ipc_mut<F>(self, mut on_ipc: F)
    where
        F: FnMut(String, &wry::WebView, &mut wry::Rect),
    {
        let queue = self.ipc_queue.clone()
            .expect("run_with_ipc_mut requires Overlay::new_with_ipc_queue");

        let init_size = discord_client_size(self.discord_hwnd).unwrap_or((720, 480));
        let mut current_bounds = wry::Rect {
            position: wry::dpi::PhysicalPosition::new(0, 0).into(),
            size: wry::dpi::PhysicalSize::new(init_size.0, init_size.1).into(),
        };

        self.run_loop(false, |webview, _discord_hwnd| {
            let msgs: Vec<String> = queue.lock()
                .map(|mut q| q.drain(..).collect())
                .unwrap_or_default();

            for msg in msgs {
                on_ipc(msg, webview, &mut current_bounds);
            }
        });
    }

    /// Core loop shared by `run`, `run_with_ipc`, and `run_with_ipc_mut`.
    /// `sync_to_discord`: if true, the webview bounds track Discord's full client rect.
    /// `per_frame`: called once per frame after message pump and size sync.
    fn run_loop<F>(self, sync_to_discord: bool, mut per_frame: F)
    where
        F: FnMut(&wry::WebView, HWND),
    {
        let mut last_size    = discord_client_size(self.discord_hwnd).unwrap_or((0, 0));
        let mut last_mouse   = (0i32, 0i32);
        let mut last_buttons = [false; 5];

        loop {
            // --- pump messages ---
            unsafe {
                let mut msg: MSG = std::mem::zeroed();
                while PeekMessageA(&mut msg, std::ptr::null_mut(), 0, 0, PM_REMOVE) != 0 {
                    TranslateMessage(&msg);
                    DispatchMessageA(&msg);
                    if msg.message == winapi::um::winuser::WM_QUIT {
                        return;
                    }
                }
            }

            // --- Discord alive check + optional full-window size sync ---
            match discord_client_size(self.discord_hwnd) {
                None => {
                    eprintln!("Discord Overlay window lost, shutting down.");
                    return;
                }
                Some(size) if sync_to_discord && size != last_size => {
                    last_size = size;
                    let _ = self.webview.set_bounds(wry::Rect {
                        position: wry::dpi::PhysicalPosition::new(0, 0).into(),
                        size: wry::dpi::PhysicalSize::new(size.0, size.1).into(),
                    });
                }
                Some(size) => { last_size = size; }
            }

            // --- poll mouse, inject into page ---
            let (mx, my) = unsafe {
                let mut pt: POINT = std::mem::zeroed();
                GetCursorPos(&mut pt);
                ScreenToClient(self.discord_hwnd, &mut pt);
                (pt.x, pt.y)
            };

            let buttons = unsafe {[
                (GetAsyncKeyState(VK_LBUTTON)  as u16 & 0x8000) != 0,
                (GetAsyncKeyState(VK_RBUTTON)  as u16 & 0x8000) != 0,
                (GetAsyncKeyState(VK_MBUTTON)  as u16 & 0x8000) != 0,
                (GetAsyncKeyState(VK_XBUTTON1) as u16 & 0x8000) != 0,
                (GetAsyncKeyState(VK_XBUTTON2) as u16 & 0x8000) != 0,
            ]};

            const BUTTONS_BIT: [u8; 5] = [0, 1, 2, 3, 4];
            let buttons_mask: u8 = buttons.iter().enumerate()
                .filter(|(_, &down)| down)
                .fold(0u8, |acc, (i, _)| acc | (1 << BUTTONS_BIT[i]));

            const BUTTON_NUM: [u16; 5] = [0, 2, 1, 3, 4];

            let mut js = format!("window.__lastButtonsMask={buttons_mask};");

            for i in 0..5usize {
                if buttons[i] != last_buttons[i] {
                    let btn = BUTTON_NUM[i];
                    if buttons[i] {
                        // mousedown: capture element under cursor.
                        // If it's an input[type=range], also record the slider's rect
                        // so we can compute values from cursor position during drag.
                        js.push_str(&format!(
                            "(function(){{\
                               var t=document.elementFromPoint({mx},{my})||document.body;\
                               window.__capturedEl=t;\
                               if(t.tagName==='INPUT'&&t.type==='range'){{\
                                 var r=t.getBoundingClientRect();\
                                 window.__sliderDrag={{el:t,rect:r}};\
                               }}\
                               t.dispatchEvent(new MouseEvent('mousedown',\
                                 {{clientX:{mx},clientY:{my},button:{btn},\
                                   buttons:{buttons_mask},bubbles:true,cancelable:true}}));\
                             }})();"
                        ));
                    } else {
                        // mouseup: clear slider drag state, dispatch events
                        js.push_str(&format!(
                            "(function(){{\
                               window.__sliderDrag=null;\
                               var t=window.__capturedEl||document.elementFromPoint({mx},{my})||document.body;\
                               window.__capturedEl=null;\
                               t.dispatchEvent(new MouseEvent('mouseup',\
                                 {{clientX:{mx},clientY:{my},button:{btn},\
                                   buttons:{buttons_mask},bubbles:true,cancelable:true}}));\
                             }})();"
                        ));
                        if i == 0 {
                            js.push_str(&format!(
                                "(function(){{\
                                   var t=document.elementFromPoint({mx},{my})||document.body;\
                                   t.dispatchEvent(new MouseEvent('click',\
                                     {{clientX:{mx},clientY:{my},button:0,\
                                       buttons:{buttons_mask},bubbles:true,cancelable:true}}));\
                                 }})();"
                            ));
                        }
                    }
                }
            }

            // mousemove: if dragging a range input, compute and set its value directly —
            // synthetic events cannot move the slider thumb in WebView2's renderer.
            // For all other elements, dispatch mousemove to the captured/hovered element.
            if (mx, my) != last_mouse || !js.is_empty() {
                last_mouse = (mx, my);
                js.push_str(&format!(
                    "(function(){{\
                       var sd=window.__sliderDrag;\
                       if(sd&&{buttons_mask}){{\
                         var r=sd.rect;\
                         var pct=Math.max(0,Math.min(1,({mx}-r.left)/(r.width||1)));\
                         var mn=parseFloat(sd.el.min)||0;\
                         var mx2=parseFloat(sd.el.max)||100;\
                         var step=parseFloat(sd.el.step)||1;\
                         var raw=mn+pct*(mx2-mn);\
                         var val=Math.round(raw/step)*step;\
                         val=Math.max(mn,Math.min(mx2,val));\
                         if(sd.el.value!=val){{\
                           sd.el.value=val;\
                           sd.el.dispatchEvent(new Event('input',{{bubbles:true}}));\
                           sd.el.dispatchEvent(new Event('change',{{bubbles:true}}));\
                         }}\
                       }}else{{\
                         var t=(window.__capturedEl&&{buttons_mask})\
                               ?window.__capturedEl\
                               :(document.elementFromPoint({mx},{my})||document.body);\
                         t.dispatchEvent(new MouseEvent('mousemove',\
                           {{clientX:{mx},clientY:{my},buttons:{buttons_mask},\
                             bubbles:true,cancelable:true}}));\
                       }}\
                     }})();"
                ));
            }

            if !js.is_empty() {
                let _ = self.webview.evaluate_script(&js);
            }

            last_buttons = buttons;

            // --- per-frame callback (IPC drain, etc.) ---
            per_frame(&self.webview, self.discord_hwnd);

            unsafe { winapi::um::synchapi::Sleep(1); }
        }
    }

    /// Run the overlay loop, syncing the webview to Discord's full client rect each frame.
    pub fn run(self) {
        self.run_loop(true, |_, _| {});
    }
}
