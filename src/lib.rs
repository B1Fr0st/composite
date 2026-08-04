//! Owned overlay renderer combining Wry, ImGui, and D3D11.
//!
//! Layer order is deterministic:
//! 1. caller primitives submitted through the underlay draw list;
//! 2. the captured Wry/WebView2 texture;
//! 3. ordinary ImGui windows and the ImGui foreground draw list.

#![cfg_attr(not(windows), allow(dead_code))]

#[cfg(not(windows))]
compile_error!("newoverlay currently supports Windows only");

pub mod webview_texture;

use std::{
    path::PathBuf,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use imgui::{Context, DrawListMut, FontSource, TextureId, Ui};
use imgui_dx11_renderer::Renderer;
use thiserror::Error;
use webview_texture::{FrameInfo, WebViewTexture, WebViewTextureBuilder};
use windows::{
    Win32::{
        Foundation::{HINSTANCE, HMODULE, HWND, LPARAM, LRESULT, POINT, RECT, WPARAM},
        Graphics::{
            Direct3D::D3D_DRIVER_TYPE_HARDWARE,
            Direct3D11::{
                D3D11_BIND_FLAG, D3D11_BIND_RENDER_TARGET, D3D11_BIND_SHADER_RESOURCE,
                D3D11_CPU_ACCESS_READ, D3D11_CREATE_DEVICE_BGRA_SUPPORT, D3D11_MAP_READ,
                D3D11_MAPPED_SUBRESOURCE, D3D11_RESOURCE_MISC_FLAG, D3D11_SDK_VERSION,
                D3D11_TEXTURE2D_DESC, D3D11_USAGE_DEFAULT, D3D11_USAGE_STAGING, D3D11CreateDevice,
                ID3D11Device, ID3D11DeviceContext, ID3D11RenderTargetView,
                ID3D11ShaderResourceView, ID3D11Texture2D,
            },
            DirectComposition::{
                DCompositionCreateDevice, IDCompositionDevice, IDCompositionTarget,
                IDCompositionVisual,
            },
            Dxgi::{
                Common::{
                    DXGI_ALPHA_MODE_PREMULTIPLIED, DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_FORMAT_UNKNOWN,
                    DXGI_SAMPLE_DESC,
                },
                DXGI_PRESENT, DXGI_SCALING_STRETCH, DXGI_SWAP_CHAIN_DESC1, DXGI_SWAP_CHAIN_FLAG,
                DXGI_SWAP_EFFECT_FLIP_SEQUENTIAL, DXGI_USAGE_RENDER_TARGET_OUTPUT, IDXGIDevice,
                IDXGIFactory2, IDXGISwapChain1,
            },
            Gdi::{ClientToScreen, ScreenToClient},
        },
        System::LibraryLoader::GetModuleHandleW,
        UI::{
            Input::KeyboardAndMouse::{
                GetAsyncKeyState, VK_LBUTTON, VK_MBUTTON, VK_RBUTTON, VK_XBUTTON1, VK_XBUTTON2,
            },
            WindowsAndMessaging::{
                CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, GetClientRect,
                GetCursorPos, GetForegroundWindow, HWND_TOPMOST, IsIconic, IsWindow,
                IsWindowVisible, MSG, PM_REMOVE, PeekMessageW, RegisterClassW, SW_HIDE,
                SW_SHOWNOACTIVATE, SWP_NOACTIVATE, SWP_NOOWNERZORDER, SWP_SHOWWINDOW, SetWindowPos,
                ShowWindow, TranslateMessage, WM_QUIT, WNDCLASSW, WS_EX_NOACTIVATE,
                WS_EX_NOREDIRECTIONBITMAP, WS_EX_TOOLWINDOW, WS_EX_TRANSPARENT, WS_POPUP,
            },
        },
    },
    core::{Interface, PCWSTR},
};

pub use imgui;
pub use wry;

/// Messages received from WebView JavaScript through `window.ipc.postMessage`.
pub type IpcHandler = Box<dyn Fn(String) + Send + 'static>;
/// Native callback for WebView navigation lifecycle events.
pub type PageLoadHandler = Box<dyn Fn(wry::PageLoadEvent, String) + 'static>;

/// Configuration for [`Overlay`].
pub struct OverlayConfig {
    /// Request a transparent WebView background, allowing underlay primitives
    /// to show through transparent page regions.
    pub transparent: bool,
    /// Enable the WebView2 devtools APIs.
    pub devtools: bool,
    /// Include the operating-system cursor in the raw WebView capture.
    pub capture_cursor: bool,
    /// Do not inject mouse events into WebView while ImGui wants the mouse.
    pub respect_imgui_mouse_capture: bool,
    /// Color used to clear the combined texture before drawing its layers.
    ///
    /// Keep alpha at `0.0` for a transparent overlay. An alpha of
    /// `1.0` intentionally makes every otherwise-empty pixel opaque.
    pub clear_color: [f32; 4],
    /// Optional WebView IPC callback.
    pub ipc_handler: Option<IpcHandler>,
    /// JavaScript injected before page scripts on every navigation.
    pub initialization_script: Option<String>,
    /// Optional callback for navigation start and finish events.
    pub page_load_handler: Option<PageLoadHandler>,
    /// Persistent WebView2 user-data directory. `None` uses an in-memory profile.
    pub data_directory: Option<PathBuf>,
    /// Run the WebView in incognito mode (no persistent storage).
    pub incognito: bool,
    /// Extra Chromium command-line switches passed to WebView2.
    pub additional_browser_args: Option<String>,
}

impl Default for OverlayConfig {
    fn default() -> Self {
        Self {
            transparent: true,
            devtools: cfg!(debug_assertions),
            capture_cursor: false,
            respect_imgui_mouse_capture: true,
            clear_color: [0.0, 0.0, 0.0, 0.0],
            ipc_handler: None,
            initialization_script: None,
            page_load_handler: None,
            data_directory: None,
            incognito: false,
            additional_browser_args: None,
        }
    }
}

/// Errors returned by the D3D11/WebView/ImGui overlay pipeline.
#[derive(Debug, Error)]
pub enum OverlayError {
    #[error("the foreground target window was not found")]
    DiscordWindowNotFound,
    #[error("the foreground target window has an invalid client size")]
    InvalidWindowSize,
    #[error("a Windows API returned no object for {0}")]
    MissingObject(&'static str),
    #[error("WebView texture error: {0}")]
    WebView(#[from] webview_texture::Error),
    #[error("Windows API error: {0}")]
    Windows(#[from] windows::core::Error),
    #[error("ImGui D3D11 renderer error: {0}")]
    ImGuiRenderer(String),
}

/// Result of one composed overlay frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct OverlayFrame {
    /// Metadata for the combined texture.
    pub texture: FrameInfo,
    /// Whether WebView2 supplied a new compositor frame this iteration.
    pub webview_updated: bool,
}

/// Ready-to-use owned overlay renderer.
pub struct Overlay {
    target_hwnd: HWND,
    d3d: D3d11State,
    presenter: CompositionPresenter,
    owned_window: OwnedOverlayWindow,
    webview: WebViewTexture,
    imgui: Context,
    renderer: Renderer,
    layers: LayerResources,
    ipc_queue: Option<Arc<Mutex<Vec<String>>>>,
    clear_color: [f32; 4],
    respect_imgui_mouse_capture: bool,
    last_frame: Instant,
    last_mouse: [i32; 2],
    last_buttons: [bool; 5],
    active: bool,
    debug: DebugTelemetry,
}

struct DebugTelemetry {
    enabled: bool,
    last_report: Instant,
    frames: u64,
    webview_updates: u64,
}

impl Overlay {
    /// Capture the foreground target and initialize the owned window, Wry,
    /// ImGui, and D3D11.
    pub fn new(config: OverlayConfig) -> Result<Self, OverlayError> {
        Self::build(config, None)
    }

    /// Construct an overlay with queue-based IPC delivery for
    /// [`Self::run_with_ipc`] and [`Self::run_with_ipc_mut`].
    pub fn new_with_ipc_queue(mut config: OverlayConfig) -> Result<Self, OverlayError> {
        let queue = Arc::new(Mutex::new(Vec::new()));
        let callback_queue = Arc::clone(&queue);
        config.ipc_handler = Some(Box::new(move |message| {
            if let Ok(mut messages) = callback_queue.lock() {
                messages.push(message);
            }
        }));
        Self::build(config, Some(queue))
    }

    fn build(
        config: OverlayConfig,
        ipc_queue: Option<Arc<Mutex<Vec<String>>>>,
    ) -> Result<Self, OverlayError> {
        let target_hwnd = unsafe { GetForegroundWindow() };
        if target_hwnd.is_invalid() {
            return Err(OverlayError::DiscordWindowNotFound);
        }
        let (x, y, width, height) =
            target_client_bounds(target_hwnd).ok_or(OverlayError::InvalidWindowSize)?;
        let mut owned_window = OwnedOverlayWindow::new(x, y, width, height)?;
        let d3d = D3d11State::new()?;

        let mut web_context = wry::WebContext::new(config.data_directory);
        let mut builder = wry::WebViewBuilder::new_with_web_context(&mut web_context)
            .with_transparent(config.transparent)
            .with_devtools(config.devtools)
            .with_incognito(config.incognito);
        use wry::WebViewBuilderExtWindows;
        if let Some(args) = config.additional_browser_args {
            builder = builder.with_additional_browser_args(args);
        }
        if let Some(script) = config.initialization_script {
            builder = builder.with_initialization_script(script);
        }
        if let Some(handler) = config.page_load_handler {
            builder = builder.with_on_page_load_handler(move |event, url| handler(event, url));
        }
        if let Some(handler) = config.ipc_handler {
            builder = builder.with_ipc_handler(move |request| {
                handler(request.body().to_string());
            });
        }
        let webview = WebViewTextureBuilder::new(builder, width, height)
            .with_cursor_capture(config.capture_cursor)
            .build(&d3d.device)?;

        let mut imgui = Context::create();
        imgui.set_ini_filename(None);
        imgui.style_mut().use_dark_colors();
        imgui.io_mut().display_size = [width as f32, height as f32];
        let renderer_device = bridge_renderer_device(&d3d.device);
        let mut renderer = unsafe { Renderer::new(&mut imgui, &renderer_device) }
            .map_err(|error| OverlayError::ImGuiRenderer(error.to_string()))?;
        let layers = LayerResources::new(&d3d.device, &mut renderer, width, height, None)?;
        let presenter = CompositionPresenter::new(&d3d.device, owned_window.hwnd, width, height)?;
        owned_window.set_visible(true);
        Ok(Self {
            target_hwnd,
            d3d,
            presenter,
            owned_window,
            webview,
            imgui,
            renderer,
            layers,
            ipc_queue,
            clear_color: config.clear_color,
            respect_imgui_mouse_capture: config.respect_imgui_mouse_capture,
            last_frame: Instant::now(),
            last_mouse: [-1, -1],
            last_buttons: [false; 5],
            active: true,
            debug: DebugTelemetry {
                enabled: std::env::var_os("COMPOSITE_DEBUG").is_some(),
                last_report: Instant::now(),
                frames: 0,
                webview_updates: 0,
            },
        })
    }

    /// Pump messages, synchronize target geometry/focus, and update input.
    pub fn start_render(&mut self) -> bool {
        unsafe {
            let mut message = MSG::default();
            while PeekMessageW(&mut message, None, 0, 0, PM_REMOVE).as_bool() {
                if message.message == WM_QUIT {
                    return false;
                }
                let _ = TranslateMessage(&message);
                DispatchMessageW(&message);
            }
        }

        if !unsafe { IsWindow(Some(self.target_hwnd)).as_bool() } {
            return false;
        }
        let Some((x, y, width, height)) = target_client_bounds(self.target_hwnd) else {
            self.active = false;
            self.owned_window.set_visible(false);
            return true;
        };
        let target_active = unsafe {
            GetForegroundWindow() == self.target_hwnd
                && IsWindowVisible(self.target_hwnd).as_bool()
                && !IsIconic(self.target_hwnd).as_bool()
        };
        self.owned_window.sync(x, y, width, height, target_active);
        self.active = target_active;
        if !target_active {
            self.last_frame = Instant::now();
            return true;
        }
        if (width, height) != self.window_size() && self.resize(width, height).is_err() {
            return false;
        }

        let mut point = POINT::default();
        unsafe {
            if GetCursorPos(&mut point).is_err()
                || !ScreenToClient(self.owned_window.hwnd, &mut point).as_bool()
            {
                return false;
            }
        }
        let mouse = [point.x, point.y];
        let buttons = unsafe {
            [
                key_down(VK_LBUTTON.0),
                key_down(VK_RBUTTON.0),
                key_down(VK_MBUTTON.0),
                key_down(VK_XBUTTON1.0),
                key_down(VK_XBUTTON2.0),
            ]
        };

        let now = Instant::now();
        let delta = now - self.last_frame;
        self.last_frame = now;
        let capture_mouse = self.imgui.io().want_capture_mouse;
        let io = self.imgui.io_mut();
        io.display_size = [width as f32, height as f32];
        io.delta_time = delta.as_secs_f32().max(f32::EPSILON);
        io.mouse_pos = [mouse[0] as f32, mouse[1] as f32];
        io.mouse_down = buttons;

        if !self.respect_imgui_mouse_capture || !capture_mouse {
            inject_webview_mouse(&self.webview, mouse, buttons, self.last_buttons);
        }
        self.last_mouse = mouse;
        self.last_buttons = buttons;
        true
    }

    /// Render a frame with caller primitives explicitly beneath the WebView.
    ///
    /// Commands added to `underlay` are emitted before the WebView image.
    /// Ordinary ImGui windows created from `ui`, plus the foreground draw list,
    /// remain above the WebView.
    pub fn render<F>(&mut self, draw: F) -> Result<OverlayFrame, OverlayError>
    where
        F: FnOnce(&Ui, &DrawListMut<'_>),
    {
        if !self.active {
            std::thread::sleep(Duration::from_millis(16));
            let (width, height) = self.window_size();
            return Ok(OverlayFrame {
                texture: FrameInfo {
                    width,
                    height,
                    generation: self.layers.generation,
                },
                webview_updated: false,
            });
        }
        let webview_updated = if let Some(frame) = self.webview.try_render()? {
            unsafe {
                self.d3d
                    .context
                    .CopyResource(&self.layers.webview, self.webview.texture());
            }
            self.layers.has_webview_frame = true;
            self.layers.generation = frame.generation;
            true
        } else {
            false
        };

        unsafe {
            self.d3d
                .context
                .OMSetRenderTargets(Some(&[Some(self.layers.combined_rtv.clone())]), None);
            self.d3d
                .context
                .ClearRenderTargetView(&self.layers.combined_rtv, &self.clear_color);
        }

        let [width, height] = self.imgui.io().display_size;
        let ui = self.imgui.frame();
        let underlay = ui.get_background_draw_list();
        draw(&ui, &underlay);
        if self.layers.has_webview_frame {
            underlay
                .add_image(self.layers.webview_texture_id, [0.0, 0.0], [width, height])
                .build();
        }
        drop(underlay);
        let draw_data = ui.render();
        let draw_lists = draw_data.draw_lists_count();
        let vertices = draw_data.total_vtx_count;
        let indices = draw_data.total_idx_count;
        self.renderer
            .render(draw_data)
            .map_err(|error| OverlayError::ImGuiRenderer(error.to_string()))?;

        unsafe {
            self.d3d.context.OMSetRenderTargets(None, None);
            self.d3d.context.Flush();
        }
        self.presenter
            .present(&self.d3d.context, &self.layers.combined)?;

        if self.debug.enabled {
            self.debug.frames += 1;
            self.debug.webview_updates += u64::from(webview_updated);
            if self.debug.last_report.elapsed() >= Duration::from_secs(1) {
                let pixels = debug_texture_stats(
                    &self.d3d.device,
                    &self.d3d.context,
                    &self.layers.combined,
                    width as u32,
                    height as u32,
                );
                eprintln!(
                    "[composite debug] render frames={} webview_updates={} generation={} size={}x{} has_webview={} draw_lists={} vertices={} indices={} present=ok pixels={pixels:?}",
                    self.debug.frames,
                    self.debug.webview_updates,
                    self.layers.generation,
                    width as u32,
                    height as u32,
                    self.layers.has_webview_frame,
                    draw_lists,
                    vertices,
                    indices,
                );
                self.debug.frames = 0;
                self.debug.webview_updates = 0;
                self.debug.last_report = Instant::now();
            }
        }

        Ok(OverlayFrame {
            texture: FrameInfo {
                width: width as u32,
                height: height as u32,
                generation: self.layers.generation,
            },
            webview_updated,
        })
    }

    /// Resize all WebView capture, composition, and swap-chain resources.
    pub fn resize(&mut self, width: u32, height: u32) -> Result<(), OverlayError> {
        if width == 0 || height == 0 {
            return Err(OverlayError::InvalidWindowSize);
        }
        self.presenter.resize(width, height)?;
        self.webview.resize(width, height)?;
        let texture_id = self.layers.webview_texture_id;
        self.layers = LayerResources::new(
            &self.d3d.device,
            &mut self.renderer,
            width,
            height,
            Some(texture_id),
        )?;
        self.imgui.io_mut().display_size = [width as f32, height as f32];
        Ok(())
    }

    /// Set the combined texture clear color.
    pub fn set_clear_color(&mut self, color: [f32; 4]) {
        self.clear_color = color;
    }

    /// The final D3D11 texture containing underlay + WebView + ImGui UI.
    pub fn texture(&self) -> &ID3D11Texture2D {
        &self.layers.combined
    }

    /// Shader-resource view for [`Self::texture`].
    pub fn shader_resource_view(&self) -> &ID3D11ShaderResourceView {
        &self.layers.combined_srv
    }

    /// The captured WebView texture before ImGui composition.
    pub fn webview_texture(&self) -> &ID3D11Texture2D {
        self.webview.texture()
    }

    /// Access the underlying Wry WebView.
    pub fn webview(&self) -> &wry::WebView {
        self.webview.webview()
    }

    /// Load a URL into the WebView.
    pub fn load_url(&self, url: &str) {
        let _ = self.webview().load_url(url);
    }

    /// Load an HTML document into the WebView.
    pub fn load_html(&self, html: &str) {
        let _ = self.webview().load_html(html);
    }

    /// Evaluate JavaScript in the WebView.
    pub fn eval(&self, script: &str) {
        let _ = self.webview().evaluate_script(script);
    }

    /// Physical dimensions of the combined texture.
    pub fn window_size(&self) -> (u32, u32) {
        (self.layers.width, self.layers.height)
    }

    /// Rebuild the font atlas and ImGui renderer after adding fonts.
    pub fn add_fonts(&mut self, sources: &[FontSource<'_>]) -> Result<(), OverlayError> {
        for source in sources {
            self.imgui.fonts().add_font(std::slice::from_ref(source));
        }
        self.rebuild_renderer()
    }

    /// Advanced font configuration followed by renderer rebuild.
    pub fn configure_fonts<F>(&mut self, configure: F) -> Result<(), OverlayError>
    where
        F: FnOnce(&mut Context),
    {
        configure(&mut self.imgui);
        self.rebuild_renderer()
    }

    fn rebuild_renderer(&mut self) -> Result<(), OverlayError> {
        let renderer_device = bridge_renderer_device(&self.d3d.device);
        self.renderer = unsafe { Renderer::new(&mut self.imgui, &renderer_device) }
            .map_err(|error| OverlayError::ImGuiRenderer(error.to_string()))?;
        let old_srv = bridge_renderer_srv(&self.layers.webview_srv);
        self.layers.webview_texture_id = self.renderer.textures_mut().insert(old_srv);
        Ok(())
    }

    /// Run until the target or message loop exits, without custom drawing.
    pub fn run(mut self) {
        while self.start_render() {
            if self.render(|_, _| {}).is_err() {
                break;
            }
        }
    }

    /// Run and deliver queued IPC messages every frame.
    pub fn run_with_ipc<F>(mut self, mut on_ipc: F)
    where
        F: FnMut(String, &wry::WebView, wry::Rect),
    {
        let bounds = self.full_bounds();
        while self.start_render() {
            for message in self.drain_ipc() {
                on_ipc(message, self.webview(), bounds);
            }
            if self.render(|_, _| {}).is_err() {
                break;
            }
        }
    }

    /// Run and deliver queued IPC messages with mutable logical bounds.
    pub fn run_with_ipc_mut<F>(mut self, mut on_ipc: F)
    where
        F: FnMut(String, &wry::WebView, &mut wry::Rect),
    {
        let mut bounds = self.full_bounds();
        while self.start_render() {
            for message in self.drain_ipc() {
                on_ipc(message, self.webview(), &mut bounds);
            }
            if self.render(|_, _| {}).is_err() {
                break;
            }
        }
    }

    fn full_bounds(&self) -> wry::Rect {
        let (width, height) = self.window_size();
        wry::Rect {
            position: wry::dpi::PhysicalPosition::new(0, 0).into(),
            size: wry::dpi::PhysicalSize::new(width, height).into(),
        }
    }

    fn drain_ipc(&self) -> Vec<String> {
        self.ipc_queue
            .as_ref()
            .and_then(|queue| {
                queue
                    .lock()
                    .ok()
                    .map(|mut messages| messages.drain(..).collect())
            })
            .unwrap_or_default()
    }
}

fn debug_texture_stats(
    device: &ID3D11Device,
    context: &ID3D11DeviceContext,
    texture: &ID3D11Texture2D,
    width: u32,
    height: u32,
) -> Result<(u64, u64, u8, u8), windows::core::Error> {
    let description = D3D11_TEXTURE2D_DESC {
        Width: width,
        Height: height,
        MipLevels: 1,
        ArraySize: 1,
        Format: DXGI_FORMAT_B8G8R8A8_UNORM,
        SampleDesc: DXGI_SAMPLE_DESC {
            Count: 1,
            Quality: 0,
        },
        Usage: D3D11_USAGE_STAGING,
        BindFlags: 0,
        CPUAccessFlags: D3D11_CPU_ACCESS_READ.0 as u32,
        MiscFlags: 0,
    };
    let mut staging = None;
    unsafe { device.CreateTexture2D(&description, None, Some(&mut staging))? };
    let staging = staging.ok_or_else(windows::core::Error::from_win32)?;
    unsafe { context.CopyResource(&staging, texture) };
    let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
    unsafe { context.Map(&staging, 0, D3D11_MAP_READ, 0, Some(&mut mapped))? };
    let mut nonzero_alpha = 0_u64;
    let mut nonzero_rgb = 0_u64;
    let mut min_alpha = u8::MAX;
    let mut max_alpha = u8::MIN;
    for y in 0..height as usize {
        let row = unsafe {
            std::slice::from_raw_parts(
                (mapped.pData as *const u8).add(y * mapped.RowPitch as usize),
                width as usize * 4,
            )
        };
        for pixel in row.chunks_exact(4) {
            nonzero_rgb += u64::from(pixel[0] != 0 || pixel[1] != 0 || pixel[2] != 0);
            nonzero_alpha += u64::from(pixel[3] != 0);
            min_alpha = min_alpha.min(pixel[3]);
            max_alpha = max_alpha.max(pixel[3]);
        }
    }
    unsafe { context.Unmap(&staging, 0) };
    Ok((nonzero_rgb, nonzero_alpha, min_alpha, max_alpha))
}

struct LayerResources {
    width: u32,
    height: u32,
    generation: u64,
    has_webview_frame: bool,
    webview: ID3D11Texture2D,
    webview_srv: ID3D11ShaderResourceView,
    webview_texture_id: TextureId,
    combined: ID3D11Texture2D,
    combined_rtv: ID3D11RenderTargetView,
    combined_srv: ID3D11ShaderResourceView,
}

impl LayerResources {
    fn new(
        device: &ID3D11Device,
        renderer: &mut Renderer,
        width: u32,
        height: u32,
        existing_texture_id: Option<TextureId>,
    ) -> Result<Self, OverlayError> {
        let (webview, webview_srv, _) =
            create_layer_texture(device, width, height, D3D11_BIND_SHADER_RESOURCE.0 as u32)?;
        let (combined, combined_srv, combined_rtv) = create_layer_texture(
            device,
            width,
            height,
            (D3D11_BIND_SHADER_RESOURCE | D3D11_BIND_RENDER_TARGET).0 as u32,
        )?;
        let renderer_srv = bridge_renderer_srv(&webview_srv);
        let webview_texture_id = if let Some(id) = existing_texture_id {
            let _ = renderer.textures_mut().replace(id, renderer_srv);
            id
        } else {
            renderer.textures_mut().insert(renderer_srv)
        };
        Ok(Self {
            width,
            height,
            generation: 0,
            has_webview_frame: false,
            webview,
            webview_srv,
            webview_texture_id,
            combined,
            combined_rtv: combined_rtv
                .ok_or(OverlayError::MissingObject("combined render-target view"))?,
            combined_srv,
        })
    }
}

fn create_layer_texture(
    device: &ID3D11Device,
    width: u32,
    height: u32,
    bind_flags: u32,
) -> Result<
    (
        ID3D11Texture2D,
        ID3D11ShaderResourceView,
        Option<ID3D11RenderTargetView>,
    ),
    OverlayError,
> {
    let description = D3D11_TEXTURE2D_DESC {
        Width: width,
        Height: height,
        MipLevels: 1,
        ArraySize: 1,
        Format: DXGI_FORMAT_B8G8R8A8_UNORM,
        SampleDesc: DXGI_SAMPLE_DESC {
            Count: 1,
            Quality: 0,
        },
        Usage: D3D11_USAGE_DEFAULT,
        BindFlags: D3D11_BIND_FLAG(bind_flags as i32).0 as u32,
        CPUAccessFlags: 0,
        MiscFlags: D3D11_RESOURCE_MISC_FLAG(0).0 as u32,
    };
    let mut texture = None;
    let mut srv = None;
    let mut rtv = None;
    unsafe {
        device.CreateTexture2D(&description, None, Some(&mut texture))?;
        let texture_ref = texture
            .as_ref()
            .ok_or(OverlayError::MissingObject("layer texture"))?;
        device.CreateShaderResourceView(texture_ref, None, Some(&mut srv))?;
        if bind_flags & D3D11_BIND_RENDER_TARGET.0 as u32 != 0 {
            device.CreateRenderTargetView(texture_ref, None, Some(&mut rtv))?;
        }
    }
    Ok((
        texture.ok_or(OverlayError::MissingObject("layer texture"))?,
        srv.ok_or(OverlayError::MissingObject("layer shader-resource view"))?,
        rtv,
    ))
}

struct D3d11State {
    device: ID3D11Device,
    context: ID3D11DeviceContext,
}

impl D3d11State {
    fn new() -> Result<Self, OverlayError> {
        let mut device = None;
        let mut context = None;
        unsafe {
            D3D11CreateDevice(
                None,
                D3D_DRIVER_TYPE_HARDWARE,
                HMODULE::default(),
                D3D11_CREATE_DEVICE_BGRA_SUPPORT,
                None,
                D3D11_SDK_VERSION,
                Some(&mut device),
                None,
                Some(&mut context),
            )?;
        }
        Ok(Self {
            device: device.ok_or(OverlayError::MissingObject("D3D11 device"))?,
            context: context.ok_or(OverlayError::MissingObject("D3D11 context"))?,
        })
    }
}

struct CompositionPresenter {
    swap_chain: IDXGISwapChain1,
    _composition_device: IDCompositionDevice,
    _composition_target: IDCompositionTarget,
    _composition_visual: IDCompositionVisual,
}

impl CompositionPresenter {
    fn new(
        device: &ID3D11Device,
        hwnd: HWND,
        width: u32,
        height: u32,
    ) -> Result<Self, OverlayError> {
        let dxgi_device: IDXGIDevice = device.cast()?;
        let adapter = unsafe { dxgi_device.GetAdapter()? };
        let factory: IDXGIFactory2 = unsafe { adapter.GetParent()? };
        let description = DXGI_SWAP_CHAIN_DESC1 {
            Width: width,
            Height: height,
            Format: DXGI_FORMAT_B8G8R8A8_UNORM,
            Stereo: false.into(),
            SampleDesc: DXGI_SAMPLE_DESC {
                Count: 1,
                Quality: 0,
            },
            BufferUsage: DXGI_USAGE_RENDER_TARGET_OUTPUT,
            BufferCount: 2,
            Scaling: DXGI_SCALING_STRETCH,
            SwapEffect: DXGI_SWAP_EFFECT_FLIP_SEQUENTIAL,
            AlphaMode: DXGI_ALPHA_MODE_PREMULTIPLIED,
            Flags: 0,
        };
        let swap_chain =
            unsafe { factory.CreateSwapChainForComposition(device, &description, None)? };
        let composition_device: IDCompositionDevice =
            unsafe { DCompositionCreateDevice(&dxgi_device)? };
        let composition_target = unsafe { composition_device.CreateTargetForHwnd(hwnd, true)? };
        let composition_visual = unsafe { composition_device.CreateVisual()? };
        unsafe {
            composition_visual.SetContent(&swap_chain)?;
            composition_target.SetRoot(&composition_visual)?;
            composition_device.Commit()?;
        }
        Ok(Self {
            swap_chain,
            _composition_device: composition_device,
            _composition_target: composition_target,
            _composition_visual: composition_visual,
        })
    }

    fn present(
        &self,
        context: &ID3D11DeviceContext,
        texture: &ID3D11Texture2D,
    ) -> Result<(), OverlayError> {
        let back_buffer: ID3D11Texture2D = unsafe { self.swap_chain.GetBuffer(0)? };
        unsafe {
            context.CopyResource(&back_buffer, texture);
            self.swap_chain.Present(1, DXGI_PRESENT(0)).ok()?;
        }
        Ok(())
    }

    fn resize(&self, width: u32, height: u32) -> Result<(), OverlayError> {
        unsafe {
            self.swap_chain.ResizeBuffers(
                0,
                width,
                height,
                DXGI_FORMAT_UNKNOWN,
                DXGI_SWAP_CHAIN_FLAG(0),
            )?;
        }
        Ok(())
    }
}

const OWNED_OVERLAY_CLASS: &[u16] = &[
    b'C' as u16,
    b'o' as u16,
    b'm' as u16,
    b'p' as u16,
    b'o' as u16,
    b's' as u16,
    b'i' as u16,
    b't' as u16,
    b'e' as u16,
    b'O' as u16,
    b'w' as u16,
    b'n' as u16,
    b'e' as u16,
    b'd' as u16,
    b'O' as u16,
    b'v' as u16,
    b'e' as u16,
    b'r' as u16,
    b'l' as u16,
    b'a' as u16,
    b'y' as u16,
    0,
];

struct OwnedOverlayWindow {
    hwnd: HWND,
    visible: bool,
}

impl OwnedOverlayWindow {
    fn new(x: i32, y: i32, width: u32, height: u32) -> Result<Self, OverlayError> {
        unsafe extern "system" fn window_proc(
            hwnd: HWND,
            message: u32,
            wparam: WPARAM,
            lparam: LPARAM,
        ) -> LRESULT {
            unsafe { DefWindowProcW(hwnd, message, wparam, lparam) }
        }

        let module = unsafe { GetModuleHandleW(None)? };
        let instance = HINSTANCE(module.0);
        let class = WNDCLASSW {
            lpfnWndProc: Some(window_proc),
            hInstance: instance,
            lpszClassName: PCWSTR(OWNED_OVERLAY_CLASS.as_ptr()),
            ..Default::default()
        };
        unsafe {
            RegisterClassW(&class);
        }
        let hwnd = unsafe {
            CreateWindowExW(
                WS_EX_TRANSPARENT | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE | WS_EX_NOREDIRECTIONBITMAP,
                PCWSTR(OWNED_OVERLAY_CLASS.as_ptr()),
                PCWSTR(OWNED_OVERLAY_CLASS.as_ptr()),
                WS_POPUP,
                x,
                y,
                width as i32,
                height as i32,
                None,
                None,
                Some(instance),
                None,
            )?
        };
        unsafe {
            SetWindowPos(
                hwnd,
                Some(HWND_TOPMOST),
                x,
                y,
                width as i32,
                height as i32,
                SWP_NOACTIVATE | SWP_NOOWNERZORDER | SWP_SHOWWINDOW,
            )?;
        }
        Ok(Self {
            hwnd,
            visible: true,
        })
    }

    fn sync(&mut self, x: i32, y: i32, width: u32, height: u32, visible: bool) {
        unsafe {
            let _ = SetWindowPos(
                self.hwnd,
                Some(HWND_TOPMOST),
                x,
                y,
                width as i32,
                height as i32,
                SWP_NOACTIVATE | SWP_NOOWNERZORDER,
            );
        }
        self.set_visible(visible);
    }

    fn set_visible(&mut self, visible: bool) {
        if self.visible == visible {
            return;
        }
        unsafe {
            let _ = ShowWindow(self.hwnd, if visible { SW_SHOWNOACTIVATE } else { SW_HIDE });
        }
        self.visible = visible;
    }
}

impl Drop for OwnedOverlayWindow {
    fn drop(&mut self) {
        if !self.hwnd.is_invalid() {
            let _ = unsafe { DestroyWindow(self.hwnd) };
        }
    }
}

fn target_client_bounds(hwnd: HWND) -> Option<(i32, i32, u32, u32)> {
    let mut rect = RECT::default();
    unsafe { GetClientRect(hwnd, &mut rect).ok()? };
    let width = (rect.right - rect.left).max(0) as u32;
    let height = (rect.bottom - rect.top).max(0) as u32;
    if width == 0 || height == 0 {
        return None;
    }
    let mut origin = POINT { x: 0, y: 0 };
    unsafe {
        if !ClientToScreen(hwnd, &mut origin).as_bool() {
            return None;
        }
    }
    Some((origin.x, origin.y, width, height))
}

unsafe fn key_down(key: u16) -> bool {
    unsafe { GetAsyncKeyState(key as i32) as u16 & 0x8000 != 0 }
}

fn inject_webview_mouse(
    webview: &WebViewTexture,
    mouse: [i32; 2],
    buttons: [bool; 5],
    previous: [bool; 5],
) {
    let mask = buttons
        .iter()
        .enumerate()
        .fold(0_u8, |value, (index, down)| {
            value | ((*down as u8) << index)
        });
    let button_numbers = [0, 2, 1, 3, 4];
    let mut script = format!(
        "(function(){{var x={},y={},m={};var t=window.__newoverlayCapture||document.elementFromPoint(x,y)||document.body;",
        mouse[0], mouse[1], mask
    );
    script.push_str(
        "t.dispatchEvent(new MouseEvent('mousemove',{clientX:x,clientY:y,buttons:m,bubbles:true,cancelable:true}));",
    );
    for index in 0..buttons.len() {
        if buttons[index] != previous[index] {
            let event = if buttons[index] {
                "mousedown"
            } else {
                "mouseup"
            };
            if buttons[index] {
                script.push_str("window.__newoverlayCapture=t;");
            }
            script.push_str(&format!(
                "t.dispatchEvent(new MouseEvent('{event}',{{clientX:x,clientY:y,button:{},buttons:m,bubbles:true,cancelable:true}}));",
                button_numbers[index]
            ));
            if !buttons[index] {
                if index == 0 {
                    script.push_str(
                        "t.dispatchEvent(new MouseEvent('click',{clientX:x,clientY:y,button:0,buttons:m,bubbles:true,cancelable:true}));",
                    );
                }
                script.push_str("window.__newoverlayCapture=null;");
            }
        }
    }
    script.push_str("})();");
    let _ = webview.webview().evaluate_script(&script);
}

fn bridge_renderer_device(
    device: &ID3D11Device,
) -> windows_036::Win32::Graphics::Direct3D11::ID3D11Device {
    let raw = device.clone().into_raw();
    unsafe {
        <windows_036::Win32::Graphics::Direct3D11::ID3D11Device as windows_036::core::Abi>::from_abi(raw)
            .expect("the transferred D3D11 device pointer is non-null")
    }
}

fn bridge_renderer_srv(
    srv: &ID3D11ShaderResourceView,
) -> windows_036::Win32::Graphics::Direct3D11::ID3D11ShaderResourceView {
    let raw = srv.clone().into_raw();
    unsafe {
        <windows_036::Win32::Graphics::Direct3D11::ID3D11ShaderResourceView as windows_036::core::Abi>::from_abi(raw)
            .expect("the transferred D3D11 SRV pointer is non-null")
    }
}
