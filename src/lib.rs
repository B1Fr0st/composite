//! Discord overlay renderer combining Wry, ImGui, and D3D11.
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
    time::Instant,
};

use imgui::{Context, DrawListMut, FontSource, TextureId, Ui};
use imgui_dx11_renderer::Renderer;
use thiserror::Error;
use winapi::{
    shared::{d3d9::*, d3d9caps::*, d3d9types::*, winerror::FAILED},
    um::winnt::HANDLE as WinApiHandle,
};
use windows::{
    Win32::{
        Foundation::{HMODULE, HWND, POINT, RECT},
        Graphics::{
            Direct3D::D3D_DRIVER_TYPE_HARDWARE,
            Direct3D11::{
                D3D11_BIND_FLAG, D3D11_BIND_RENDER_TARGET, D3D11_BIND_SHADER_RESOURCE,
                D3D11_CREATE_DEVICE_BGRA_SUPPORT, D3D11_RESOURCE_MISC_FLAG,
                D3D11_RESOURCE_MISC_SHARED, D3D11_SDK_VERSION, D3D11_TEXTURE2D_DESC,
                D3D11_USAGE_DEFAULT, D3D11CreateDevice, ID3D11Device, ID3D11DeviceContext,
                ID3D11RenderTargetView, ID3D11ShaderResourceView, ID3D11Texture2D,
            },
            Dxgi::{
                Common::{DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_SAMPLE_DESC},
                IDXGIResource,
            },
            Gdi::ScreenToClient,
        },
        UI::{
            Input::KeyboardAndMouse::{
                GetAsyncKeyState, VK_LBUTTON, VK_MBUTTON, VK_RBUTTON, VK_XBUTTON1, VK_XBUTTON2,
            },
            WindowsAndMessaging::{
                DispatchMessageW, FindWindowA, GetClientRect, GetCursorPos, MSG, PM_REMOVE,
                PeekMessageW, TranslateMessage, WM_QUIT,
            },
        },
    },
    core::{Interface, PCSTR},
};
use wio::com::ComPtr;

use webview_texture::{FrameInfo, WebViewTexture, WebViewTextureBuilder};

pub use imgui;
pub use wry;

/// Messages received from WebView JavaScript through `window.ipc.postMessage`.
pub type IpcHandler = Box<dyn Fn(String) + Send + 'static>;

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
    /// Keep alpha at `0.0` for a transparent Discord overlay. An alpha of
    /// `1.0` intentionally makes every otherwise-empty pixel opaque.
    pub clear_color: [f32; 4],
    /// Optional WebView IPC callback.
    pub ipc_handler: Option<IpcHandler>,
    /// Custom user-data directory for the WebView2 environment.
    /// When `None`, WebView2 uses its default location next to the executable.
    pub data_directory: Option<PathBuf>,
    /// Run the WebView in incognito / in-private mode.
    pub incognito: bool,
    /// Extra Chromium command-line flags passed to the WebView2 environment.
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
            data_directory: None,
            incognito: false,
            additional_browser_args: None,
        }
    }
}

/// Errors returned by the D3D11/WebView/ImGui overlay pipeline.
#[derive(Debug, Error)]
pub enum OverlayError {
    #[error("Discord Overlay window was not found")]
    DiscordWindowNotFound,
    #[error("Discord Overlay has an invalid client size")]
    InvalidWindowSize,
    #[error("a Windows API returned no object for {0}")]
    MissingObject(&'static str),
    #[error("WebView texture error: {0}")]
    WebView(#[from] webview_texture::Error),
    #[error("Windows API error: {0}")]
    Windows(#[from] windows::core::Error),
    #[error("ImGui D3D11 renderer error: {0}")]
    ImGuiRenderer(String),
    #[error("D3D9 {operation} failed with HRESULT 0x{hresult:08X}")]
    D3d9 {
        operation: &'static str,
        hresult: u32,
    },
}

/// Result of one composed overlay frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct OverlayFrame {
    /// Metadata for the combined texture.
    pub texture: FrameInfo,
    /// Whether WebView2 supplied a new compositor frame this iteration.
    pub webview_updated: bool,
}

/// Ready-to-use Discord overlay renderer.
pub struct Overlay {
    hwnd: HWND,
    d3d: D3d11State,
    presenter: D3d9Presenter,
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
}

impl Overlay {
    /// Find Discord's overlay window and initialize Wry, ImGui, and D3D11.
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
        let hwnd = find_discord_window()?;
        let (width, height) = client_size(hwnd).ok_or(OverlayError::InvalidWindowSize)?;
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
        let presenter = D3d9Presenter::new(hwnd, width, height, &layers.combined)?;
        Ok(Self {
            hwnd,
            d3d,
            presenter,
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
        })
    }

    /// Pump messages, synchronize Discord's size, and update ImGui/WebView input.
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

        let Some((width, height)) = client_size(self.hwnd) else {
            return false;
        };
        if (width, height) != self.window_size() && self.resize(width, height).is_err() {
            return false;
        }

        let mut point = POINT::default();
        unsafe {
            if GetCursorPos(&mut point).is_err() || !ScreenToClient(self.hwnd, &mut point).as_bool()
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
        self.renderer
            .render(draw_data)
            .map_err(|error| OverlayError::ImGuiRenderer(error.to_string()))?;

        unsafe {
            self.d3d.context.OMSetRenderTargets(None, None);
            self.d3d.context.Flush();
        }
        self.presenter.present()?;

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
        self.presenter.release_shared_texture();
        self.presenter.reset(width, height)?;
        self.webview.resize(width, height)?;
        let texture_id = self.layers.webview_texture_id;
        self.layers = LayerResources::new(
            &self.d3d.device,
            &mut self.renderer,
            width,
            height,
            Some(texture_id),
        )?;
        self.presenter
            .open_shared_texture(&self.layers.combined, width, height)?;
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

    /// Run until Discord or the message loop exits, without custom drawing.
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
        MiscFlags: if bind_flags & D3D11_BIND_RENDER_TARGET.0 as u32 != 0 {
            D3D11_RESOURCE_MISC_SHARED.0 as u32
        } else {
            D3D11_RESOURCE_MISC_FLAG(0).0 as u32
        },
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

struct D3d9Presenter {
    _d3d: ComPtr<IDirect3D9Ex>,
    device: ComPtr<IDirect3DDevice9Ex>,
    present_params: D3DPRESENT_PARAMETERS,
    shared_texture: Option<ComPtr<IDirect3DTexture9>>,
}

impl D3d9Presenter {
    fn new(
        hwnd: HWND,
        width: u32,
        height: u32,
        texture: &ID3D11Texture2D,
    ) -> Result<Self, OverlayError> {
        let mut d3d_ptr = std::ptr::null_mut();
        check_d3d9(
            unsafe { Direct3DCreate9Ex(D3D_SDK_VERSION, &mut d3d_ptr) },
            "Direct3DCreate9Ex",
        )?;
        if d3d_ptr.is_null() {
            return Err(OverlayError::MissingObject("IDirect3D9Ex"));
        }
        let d3d = unsafe { ComPtr::from_raw(d3d_ptr) };
        let mut present_params = d3d9_present_params(hwnd, width, height);
        let mut device_ptr = std::ptr::null_mut();
        let mut result = unsafe {
            d3d.CreateDeviceEx(
                D3DADAPTER_DEFAULT,
                D3DDEVTYPE_HAL,
                hwnd.0 as _,
                D3DCREATE_HARDWARE_VERTEXPROCESSING,
                &mut present_params,
                std::ptr::null_mut(),
                &mut device_ptr,
            )
        };
        if FAILED(result) {
            result = unsafe {
                d3d.CreateDeviceEx(
                    D3DADAPTER_DEFAULT,
                    D3DDEVTYPE_HAL,
                    hwnd.0 as _,
                    D3DCREATE_SOFTWARE_VERTEXPROCESSING,
                    &mut present_params,
                    std::ptr::null_mut(),
                    &mut device_ptr,
                )
            };
        }
        check_d3d9(result, "IDirect3D9Ex::CreateDeviceEx")?;
        if device_ptr.is_null() {
            return Err(OverlayError::MissingObject("IDirect3DDevice9Ex"));
        }

        let mut presenter = Self {
            _d3d: d3d,
            device: unsafe { ComPtr::from_raw(device_ptr) },
            present_params,
            shared_texture: None,
        };
        presenter.open_shared_texture(texture, width, height)?;
        Ok(presenter)
    }

    fn open_shared_texture(
        &mut self,
        texture: &ID3D11Texture2D,
        width: u32,
        height: u32,
    ) -> Result<(), OverlayError> {
        let resource: IDXGIResource = texture.cast()?;
        let handle = unsafe { resource.GetSharedHandle()? };
        let mut shared_handle = handle.0 as WinApiHandle;
        let mut texture_ptr = std::ptr::null_mut();
        check_d3d9(
            unsafe {
                self.device.CreateTexture(
                    width,
                    height,
                    1,
                    D3DUSAGE_RENDERTARGET,
                    D3DFMT_A8R8G8B8,
                    D3DPOOL_DEFAULT,
                    &mut texture_ptr,
                    &mut shared_handle,
                )
            },
            "IDirect3DDevice9Ex::CreateTexture(shared)",
        )?;
        if texture_ptr.is_null() {
            return Err(OverlayError::MissingObject("shared IDirect3DTexture9"));
        }
        self.shared_texture = Some(unsafe { ComPtr::from_raw(texture_ptr) });
        Ok(())
    }

    fn release_shared_texture(&mut self) {
        self.shared_texture = None;
    }

    fn present(&self) -> Result<(), OverlayError> {
        let texture = self
            .shared_texture
            .as_ref()
            .ok_or(OverlayError::MissingObject("shared IDirect3DTexture9"))?;
        let mut source_ptr = std::ptr::null_mut();
        check_d3d9(
            unsafe { texture.GetSurfaceLevel(0, &mut source_ptr) },
            "IDirect3DTexture9::GetSurfaceLevel",
        )?;
        if source_ptr.is_null() {
            return Err(OverlayError::MissingObject("shared IDirect3DSurface9"));
        }
        let source = unsafe { ComPtr::from_raw(source_ptr) };

        let mut back_buffer_ptr = std::ptr::null_mut();
        check_d3d9(
            unsafe {
                self.device
                    .GetBackBuffer(0, 0, D3DBACKBUFFER_TYPE_MONO, &mut back_buffer_ptr)
            },
            "IDirect3DDevice9Ex::GetBackBuffer",
        )?;
        if back_buffer_ptr.is_null() {
            return Err(OverlayError::MissingObject("D3D9 back buffer"));
        }
        let back_buffer = unsafe { ComPtr::from_raw(back_buffer_ptr) };

        check_d3d9(
            unsafe {
                self.device.StretchRect(
                    source.as_raw(),
                    std::ptr::null(),
                    back_buffer.as_raw(),
                    std::ptr::null(),
                    D3DTEXF_NONE,
                )
            },
            "IDirect3DDevice9Ex::StretchRect",
        )?;
        check_d3d9(
            unsafe {
                self.device.PresentEx(
                    std::ptr::null(),
                    std::ptr::null(),
                    std::ptr::null_mut(),
                    std::ptr::null(),
                    0,
                )
            },
            "IDirect3DDevice9Ex::PresentEx",
        )
    }

    fn reset(&mut self, width: u32, height: u32) -> Result<(), OverlayError> {
        self.release_shared_texture();
        self.present_params.BackBufferWidth = width;
        self.present_params.BackBufferHeight = height;
        check_d3d9(
            unsafe {
                self.device
                    .ResetEx(&mut self.present_params, std::ptr::null_mut())
            },
            "IDirect3DDevice9Ex::ResetEx",
        )
    }
}

fn d3d9_present_params(hwnd: HWND, width: u32, height: u32) -> D3DPRESENT_PARAMETERS {
    D3DPRESENT_PARAMETERS {
        BackBufferWidth: width,
        BackBufferHeight: height,
        BackBufferFormat: D3DFMT_A8R8G8B8,
        BackBufferCount: 1,
        MultiSampleType: D3DMULTISAMPLE_NONE,
        MultiSampleQuality: 0,
        SwapEffect: D3DSWAPEFFECT_DISCARD,
        hDeviceWindow: hwnd.0 as _,
        Windowed: 1,
        EnableAutoDepthStencil: 0,
        AutoDepthStencilFormat: D3DFMT_UNKNOWN,
        Flags: 0,
        FullScreen_RefreshRateInHz: 0,
        PresentationInterval: D3DPRESENT_INTERVAL_IMMEDIATE,
    }
}

fn check_d3d9(result: i32, operation: &'static str) -> Result<(), OverlayError> {
    if FAILED(result) {
        Err(OverlayError::D3d9 {
            operation,
            hresult: result as u32,
        })
    } else {
        Ok(())
    }
}

fn find_discord_window() -> Result<HWND, OverlayError> {
    unsafe {
        FindWindowA(
            PCSTR(c"Chrome_WidgetWin_1".as_ptr().cast()),
            PCSTR(c"Discord Overlay".as_ptr().cast()),
        )
        .map_err(|_| OverlayError::DiscordWindowNotFound)
    }
}

fn client_size(hwnd: HWND) -> Option<(u32, u32)> {
    let mut rect = RECT::default();
    unsafe { GetClientRect(hwnd, &mut rect).ok()? };
    let width = (rect.right - rect.left).max(0) as u32;
    let height = (rect.bottom - rect.top).max(0) as u32;
    (width > 0 && height > 0).then_some((width, height))
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
