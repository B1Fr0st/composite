use std::{
    num::NonZeroIsize,
    time::{Duration, Instant},
};

use raw_window_handle::{
    HandleError, HasWindowHandle, RawWindowHandle, Win32WindowHandle, WindowHandle,
};
use thiserror::Error;
use windows::{
    Graphics::{
        Capture::{Direct3D11CaptureFramePool, GraphicsCaptureItem, GraphicsCaptureSession},
        DirectX::{Direct3D11::IDirect3DDevice, DirectXPixelFormat},
        SizeInt32,
    },
    Win32::{
        Foundation::{HINSTANCE, HWND, LPARAM, LRESULT, WPARAM},
        Graphics::{
            Direct3D11::{
                D3D11_BIND_RENDER_TARGET, D3D11_BIND_SHADER_RESOURCE, D3D11_RESOURCE_MISC_FLAG,
                D3D11_TEXTURE2D_DESC, D3D11_USAGE_DEFAULT, ID3D11Device, ID3D11DeviceContext,
                ID3D11ShaderResourceView, ID3D11Texture2D,
            },
            Dxgi::{Common::DXGI_FORMAT_B8G8R8A8_UNORM, IDXGIDevice},
        },
        System::{
            LibraryLoader::GetModuleHandleW,
            WinRT::{
                Direct3D11::{CreateDirect3D11DeviceFromDXGIDevice, IDirect3DDxgiInterfaceAccess},
                Graphics::Capture::IGraphicsCaptureItemInterop,
            },
        },
        UI::WindowsAndMessaging::{
            CS_HREDRAW, CS_VREDRAW, CreateWindowExW, DefWindowProcW, DestroyWindow,
            DispatchMessageW, FindWindowExW, MSG, PM_REMOVE, PeekMessageW, RegisterClassW,
            SW_SHOWNOACTIVATE, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOOWNERZORDER, SWP_NOZORDER,
            SetWindowPos, ShowWindow, TranslateMessage, WNDCLASSW, WS_CLIPCHILDREN,
            WS_CLIPSIBLINGS, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_POPUP,
        },
    },
    core::{Interface, PCWSTR},
};
use wry::{WebView, WebViewBuilder};

const HOST_CLASS_NAME: &[u16] = &[
    b'W' as u16,
    b'r' as u16,
    b'y' as u16,
    b'D' as u16,
    b'3' as u16,
    b'D' as u16,
    b'T' as u16,
    b'e' as u16,
    b'x' as u16,
    b't' as u16,
    b'u' as u16,
    b'r' as u16,
    b'e' as u16,
    b'H' as u16,
    b'o' as u16,
    b's' as u16,
    b't' as u16,
    0,
];

const HOST_TITLE: &[u16] = &[
    b'W' as u16,
    b'r' as u16,
    b'y' as u16,
    b' ' as u16,
    b't' as u16,
    b'e' as u16,
    b'x' as u16,
    b't' as u16,
    b'u' as u16,
    b'r' as u16,
    b'e' as u16,
    0,
];

const WRY_WEBVIEW_CLASS_NAME: &[u16] = &[
    b'W' as u16,
    b'R' as u16,
    b'Y' as u16,
    b'_' as u16,
    b'W' as u16,
    b'E' as u16,
    b'B' as u16,
    b'V' as u16,
    b'I' as u16,
    b'E' as u16,
    b'W' as u16,
    0,
];

/// Errors produced while creating or updating a texture-backed WebView.
#[derive(Debug, Error)]
pub enum Error {
    #[error("the texture size must be non-zero")]
    InvalidSize,
    #[error("Windows Graphics Capture is unavailable in this session")]
    CaptureUnsupported,
    #[error("failed to create the Wry WebView: {0}")]
    Wry(#[from] wry::Error),
    #[error("Windows API call failed: {0}")]
    Windows(#[from] windows::core::Error),
    #[error("a Windows API returned no object for {0}")]
    MissingObject(&'static str),
}

/// Metadata for the most recently copied compositor frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FrameInfo {
    /// Width of the texture in physical pixels.
    pub width: u32,
    /// Height of the texture in physical pixels.
    pub height: u32,
    /// Monotonically increasing number of frames copied into the texture.
    pub generation: u64,
}

/// Configures a [`WebViewTexture`] while leaving all web content configuration
/// to Wry's builder.
pub struct WebViewTextureBuilder<'a> {
    wry: WebViewBuilder<'a>,
    width: u32,
    height: u32,
    capture_cursor: bool,
}

impl<'a> WebViewTextureBuilder<'a> {
    /// Wrap an arbitrarily configured Wry builder.
    pub fn new(wry: WebViewBuilder<'a>, width: u32, height: u32) -> Self {
        Self {
            wry,
            width,
            height,
            capture_cursor: false,
        }
    }

    /// Include the operating-system cursor in captured frames.
    pub fn with_cursor_capture(mut self, enabled: bool) -> Self {
        self.capture_cursor = enabled;
        self
    }

    /// Build the WebView and allocate its persistent output texture on
    /// `device`. This must be called on the application's Windows UI thread.
    pub fn build(self, device: &ID3D11Device) -> Result<WebViewTexture, Error> {
        if self.width == 0 || self.height == 0 {
            return Err(Error::InvalidSize);
        }

        if !GraphicsCaptureSession::IsSupported()? {
            return Err(Error::CaptureUnsupported);
        }

        let host = HostWindow::new(self.width, self.height)?;
        let webview = self.wry.build(&host)?;
        let webview_hwnd = unsafe {
            FindWindowExW(
                Some(host.hwnd),
                None,
                PCWSTR(WRY_WEBVIEW_CLASS_NAME.as_ptr()),
                PCWSTR::null(),
            )?
        };
        let capture = CaptureState::new(
            device,
            host.hwnd,
            self.width,
            self.height,
            self.capture_cursor,
        )?;

        Ok(WebViewTexture {
            webview: Some(webview),
            webview_hwnd,
            host,
            capture,
        })
    }
}

/// A Wry WebView whose latest composed frame is copied into a reusable D3D11
/// texture.
///
/// `WebViewTexture` is intentionally not `Send`: Wry and its HWND must be used
/// from the UI thread that created them.
pub struct WebViewTexture {
    // Fields are ordered so capture closes before its source HWND is destroyed.
    // Drop also explicitly removes the WebView before either one.
    webview: Option<WebView>,
    webview_hwnd: HWND,
    capture: CaptureState,
    host: HostWindow,
}

impl WebViewTexture {
    /// Copy every currently queued capture frame into the output texture.
    ///
    /// Returns `Ok(Some(info))` if at least one new frame was copied and
    /// `Ok(None)` when the compositor has not produced a new frame yet.
    pub fn try_render(&mut self) -> Result<Option<FrameInfo>, Error> {
        self.capture.try_render()
    }

    /// Resize the host WebView, frame pool, and output texture.
    pub fn resize(&mut self, width: u32, height: u32) -> Result<(), Error> {
        if width == 0 || height == 0 {
            return Err(Error::InvalidSize);
        }

        self.host.resize(width, height)?;
        self.capture.resize(width, height)
    }

    /// The persistent BGRA8_UNORM output texture.
    ///
    /// Its identity remains stable until [`Self::resize`] is called.
    pub fn texture(&self) -> &ID3D11Texture2D {
        &self.capture.texture
    }

    /// A shader-resource view for [`Self::texture`].
    pub fn shader_resource_view(&self) -> &ID3D11ShaderResourceView {
        &self.capture.shader_resource_view
    }

    /// Access the wrapped Wry object for navigation, scripts, IPC, and other
    /// normal Wry operations.
    pub fn webview(&self) -> &WebView {
        self.webview
            .as_ref()
            .expect("WebView is present until drop")
    }

    /// Pump pending messages on the current UI thread.
    ///
    /// Applications already running a Win32, Tao, or Winit event loop normally
    /// do not need this method. It is useful for custom polling loops.
    pub fn pump_messages(&self) -> bool {
        unsafe {
            let mut message = MSG::default();
            while PeekMessageW(&mut message, None, 0, 0, PM_REMOVE).as_bool() {
                if message.message == windows::Win32::UI::WindowsAndMessaging::WM_QUIT {
                    return false;
                }
                let _ = TranslateMessage(&message);
                DispatchMessageW(&message);
            }
        }
        true
    }

    /// Dimensions and generation of the output texture.
    pub fn frame_info(&self) -> FrameInfo {
        self.capture.info()
    }

    /// The private top-level HWND captured by Windows Graphics Capture.
    ///
    /// This is primarily exposed for advanced native input forwarding.
    pub fn host_hwnd(&self) -> HWND {
        self.host.hwnd
    }

    /// The WRY_WEBVIEW child HWND. Native hosts can target this handle when
    /// forwarding mouse and keyboard input to the off-screen view.
    pub fn webview_hwnd(&self) -> HWND {
        self.webview_hwnd
    }
}

impl Drop for WebViewTexture {
    fn drop(&mut self) {
        // Wry closes its controller and child HWND here, before HostWindow's
        // Drop destroys the parent HWND.
        self.webview.take();
    }
}

struct CaptureState {
    device: ID3D11Device,
    context: ID3D11DeviceContext,
    winrt_device: IDirect3DDevice,
    frame_pool: Direct3D11CaptureFramePool,
    session: GraphicsCaptureSession,
    texture: ID3D11Texture2D,
    shader_resource_view: ID3D11ShaderResourceView,
    width: u32,
    height: u32,
    generation: u64,
    debug_enabled: bool,
    debug_last_report: Instant,
    debug_frames: u64,
    debug_wrong_size: u64,
}

impl CaptureState {
    fn new(
        device: &ID3D11Device,
        hwnd: HWND,
        width: u32,
        height: u32,
        capture_cursor: bool,
    ) -> Result<Self, Error> {
        let context = unsafe { device.GetImmediateContext()? };
        let dxgi_device: IDXGIDevice = device.cast()?;
        let inspectable = unsafe { CreateDirect3D11DeviceFromDXGIDevice(&dxgi_device)? };
        let winrt_device: IDirect3DDevice = inspectable.cast()?;

        let item = create_capture_item(hwnd)?;
        let size = SizeInt32 {
            Width: width as i32,
            Height: height as i32,
        };
        let frame_pool = Direct3D11CaptureFramePool::CreateFreeThreaded(
            &winrt_device,
            DirectXPixelFormat::B8G8R8A8UIntNormalized,
            2,
            size,
        )?;
        let session = frame_pool.CreateCaptureSession(&item)?;
        let _ = session.SetIsCursorCaptureEnabled(capture_cursor);
        let _ = session.SetIsBorderRequired(false);

        let (texture, shader_resource_view) = create_output_texture(device, width, height)?;
        session.StartCapture()?;

        Ok(Self {
            device: device.clone(),
            context,
            winrt_device,
            frame_pool,
            session,
            texture,
            shader_resource_view,
            width,
            height,
            generation: 0,
            debug_enabled: std::env::var_os("COMPOSITE_DEBUG").is_some(),
            debug_last_report: Instant::now(),
            debug_frames: 0,
            debug_wrong_size: 0,
        })
    }

    fn try_render(&mut self) -> Result<Option<FrameInfo>, Error> {
        let mut copied = false;

        // TryGetNextFrame reports an error when the non-blocking queue is
        // empty. Drain it so the texture contains the freshest available UI.
        while let Ok(frame) = self.frame_pool.TryGetNextFrame() {
            let content_size = frame.ContentSize()?;
            if content_size.Width == self.width as i32 && content_size.Height == self.height as i32
            {
                let surface = frame.Surface()?;
                let access: IDirect3DDxgiInterfaceAccess = surface.cast()?;
                let source: ID3D11Texture2D = unsafe { access.GetInterface()? };
                unsafe {
                    self.context.CopyResource(&self.texture, &source);
                }
                self.generation = self.generation.wrapping_add(1);
                self.debug_frames += 1;
                copied = true;
            } else {
                self.debug_wrong_size += 1;
            }
            let _ = frame.Close();
        }

        if self.debug_enabled && self.debug_last_report.elapsed() >= Duration::from_secs(1) {
            eprintln!(
                "[composite debug] capture copied={} wrong_size={} generation={} expected={}x{}",
                self.debug_frames, self.debug_wrong_size, self.generation, self.width, self.height,
            );
            self.debug_frames = 0;
            self.debug_wrong_size = 0;
            self.debug_last_report = Instant::now();
        }
        Ok(copied.then(|| self.info()))
    }

    fn resize(&mut self, width: u32, height: u32) -> Result<(), Error> {
        let size = SizeInt32 {
            Width: width as i32,
            Height: height as i32,
        };
        self.frame_pool.Recreate(
            &self.winrt_device,
            DirectXPixelFormat::B8G8R8A8UIntNormalized,
            2,
            size,
        )?;
        let (texture, shader_resource_view) = create_output_texture(&self.device, width, height)?;
        self.texture = texture;
        self.shader_resource_view = shader_resource_view;
        self.width = width;
        self.height = height;
        Ok(())
    }

    fn info(&self) -> FrameInfo {
        FrameInfo {
            width: self.width,
            height: self.height,
            generation: self.generation,
        }
    }
}

impl Drop for CaptureState {
    fn drop(&mut self) {
        let _ = self.session.Close();
        let _ = self.frame_pool.Close();
    }
}

fn create_capture_item(hwnd: HWND) -> Result<GraphicsCaptureItem, Error> {
    let interop = windows::core::factory::<GraphicsCaptureItem, IGraphicsCaptureItemInterop>()?;
    Ok(unsafe { interop.CreateForWindow::<GraphicsCaptureItem>(hwnd)? })
}

fn create_output_texture(
    device: &ID3D11Device,
    width: u32,
    height: u32,
) -> Result<(ID3D11Texture2D, ID3D11ShaderResourceView), Error> {
    let description = D3D11_TEXTURE2D_DESC {
        Width: width,
        Height: height,
        MipLevels: 1,
        ArraySize: 1,
        Format: DXGI_FORMAT_B8G8R8A8_UNORM,
        SampleDesc: windows::Win32::Graphics::Dxgi::Common::DXGI_SAMPLE_DESC {
            Count: 1,
            Quality: 0,
        },
        Usage: D3D11_USAGE_DEFAULT,
        BindFlags: (D3D11_BIND_SHADER_RESOURCE | D3D11_BIND_RENDER_TARGET).0 as u32,
        CPUAccessFlags: 0,
        MiscFlags: D3D11_RESOURCE_MISC_FLAG(0).0 as u32,
    };

    let mut texture = None;
    unsafe {
        device.CreateTexture2D(&description, None, Some(&mut texture))?;
    }
    let texture = texture.ok_or(Error::MissingObject("ID3D11Texture2D"))?;

    let mut view = None;
    unsafe {
        device.CreateShaderResourceView(&texture, None, Some(&mut view))?;
    }
    let view = view.ok_or(Error::MissingObject("ID3D11ShaderResourceView"))?;
    Ok((texture, view))
}

struct HostWindow {
    hwnd: HWND,
}

impl HostWindow {
    fn new(width: u32, height: u32) -> Result<Self, Error> {
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
            style: CS_HREDRAW | CS_VREDRAW,
            lpfnWndProc: Some(window_proc),
            hInstance: instance,
            lpszClassName: PCWSTR(HOST_CLASS_NAME.as_ptr()),
            ..Default::default()
        };

        // RegisterClassW returning zero is harmless when another instance has
        // already registered this process-local class.
        unsafe {
            RegisterClassW(&class);
        }

        let hwnd = unsafe {
            CreateWindowExW(
                WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
                PCWSTR(HOST_CLASS_NAME.as_ptr()),
                PCWSTR(HOST_TITLE.as_ptr()),
                WS_POPUP | WS_CLIPCHILDREN | WS_CLIPSIBLINGS,
                -32_000,
                -32_000,
                width as i32,
                height as i32,
                None,
                None,
                Some(instance),
                None,
            )?
        };
        unsafe {
            let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
        }
        Ok(Self { hwnd })
    }

    fn resize(&self, width: u32, height: u32) -> Result<(), Error> {
        unsafe {
            SetWindowPos(
                self.hwnd,
                None,
                0,
                0,
                width as i32,
                height as i32,
                SWP_NOMOVE | SWP_NOZORDER | SWP_NOOWNERZORDER | SWP_NOACTIVATE,
            )?;
        }
        Ok(())
    }
}

impl Drop for HostWindow {
    fn drop(&mut self) {
        if !self.hwnd.is_invalid() {
            let _ = unsafe { DestroyWindow(self.hwnd) };
        }
    }
}

impl HasWindowHandle for HostWindow {
    fn window_handle(&self) -> Result<WindowHandle<'_>, HandleError> {
        let value = NonZeroIsize::new(self.hwnd.0 as isize).ok_or(HandleError::Unavailable)?;
        let raw = RawWindowHandle::Win32(Win32WindowHandle::new(value));
        Ok(unsafe { WindowHandle::borrow_raw(raw) })
    }
}
