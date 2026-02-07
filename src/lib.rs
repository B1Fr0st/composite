use imgui::*;
use imgui_dx9_renderer::Renderer;
use std::mem;
use std::ptr;
use std::time::Instant;
use wio::com::ComPtr;
use winapi::shared::d3d9::*;
use winapi::shared::d3d9caps::*;
use winapi::shared::d3d9types::*;
use winapi::shared::minwindef::*;
use winapi::shared::windef::*;
use winapi::shared::winerror::*;
use winapi::um::winuser::*;

// D3D9 error codes
const D3DERR_DEVICELOST: i32 = 0x88760868_u32 as i32;
const D3DERR_DEVICENOTRESET: i32 = 0x88760869_u32 as i32;

pub struct Overlay {
    hwnd: HWND,
    d3d: Option<ComPtr<IDirect3D9>>,
    device: Option<ComPtr<IDirect3DDevice9>>,
    present_params: D3DPRESENT_PARAMETERS,
    imgui: Context,
    renderer: Option<Renderer>,
    resize_width: u32,
    resize_height: u32,
    last_frame: Instant,
    mouse_pos: [f32; 2],
    mouse_buttons: [bool; 5],
}

impl Overlay {
    pub fn new() -> Option<Self> {
        // Find Discord Overlay window
        let hwnd = unsafe {
            FindWindowA(
                b"Chrome_WidgetWin_1\0".as_ptr() as *const i8,
                b"Discord Overlay\0".as_ptr() as *const i8,
            )
        };

        if hwnd.is_null() {
            eprintln!("Failed to find discord window");
            return None;
        }

        println!("Found discord window: {:?}", hwnd);

        let mut imgui = Context::create();
        imgui.set_ini_filename(None);

        let mut overlay = Self {
            hwnd,
            d3d: None,
            device: None,
            present_params: unsafe { mem::zeroed() },
            imgui,
            renderer: None,
            resize_width: 0,
            resize_height: 0,
            last_frame: Instant::now(),
            mouse_pos: [0.0, 0.0],
            mouse_buttons: [false; 5],
        };

        // Create D3D9 device
        if !overlay.create_device() {
            eprintln!("Failed to create D3D9 device");
            return None;
        }

        // Initialize ImGui
        // Don't enable keyboard/gamepad navigation to avoid key mapping requirements
        // overlay.imgui.io_mut().config_flags |= ConfigFlags::NAV_ENABLE_KEYBOARD;
        // overlay.imgui.io_mut().config_flags |= ConfigFlags::NAV_ENABLE_GAMEPAD;

        // Set dark style
        let style = overlay.imgui.style_mut();
        style.use_dark_colors();

        // Setup display size
        let mut rect: RECT = unsafe { mem::zeroed() };
        unsafe {
            GetClientRect(hwnd, &mut rect);
            overlay.imgui.io_mut().display_size = [
                (rect.right - rect.left) as f32,
                (rect.bottom - rect.top) as f32,
            ];
        }

        // Initialize renderer
        match unsafe { Renderer::new(&mut overlay.imgui, overlay.device.clone().unwrap()) } {
            Ok(renderer) => {
                overlay.renderer = Some(renderer);
            }
            Err(e) => {
                eprintln!("Failed to create ImGui renderer: {}", e);
                return None;
            }
        }

        Some(overlay)
    }

    fn create_device(&mut self) -> bool {
        unsafe {
            // Create D3D9
            let d3d = Direct3DCreate9(D3D_SDK_VERSION);
            if d3d.is_null() {
                eprintln!("Failed to create D3D9");
                return false;
            }
            self.d3d = Some(ComPtr::from_raw(d3d));

            // Get client rect for back buffer size
            let mut rect: RECT = mem::zeroed();
            GetClientRect(self.hwnd, &mut rect);
            let width = (rect.right - rect.left) as u32;
            let height = (rect.bottom - rect.top) as u32;

            // Setup present parameters
            self.present_params.BackBufferWidth = width;
            self.present_params.BackBufferHeight = height;
            self.present_params.BackBufferFormat = D3DFMT_A8R8G8B8;
            self.present_params.BackBufferCount = 1;
            self.present_params.MultiSampleType = D3DMULTISAMPLE_NONE;
            self.present_params.MultiSampleQuality = 0;
            self.present_params.SwapEffect = D3DSWAPEFFECT_DISCARD;
            self.present_params.hDeviceWindow = self.hwnd;
            self.present_params.Windowed = TRUE;
            self.present_params.EnableAutoDepthStencil = TRUE;
            self.present_params.AutoDepthStencilFormat = D3DFMT_D16;
            self.present_params.Flags = 0;
            self.present_params.FullScreen_RefreshRateInHz = 0;
            self.present_params.PresentationInterval = D3DPRESENT_INTERVAL_IMMEDIATE;

            // Create device
            let mut device: *mut IDirect3DDevice9 = ptr::null_mut();
            let d3d_ref = self.d3d.as_ref().unwrap();
            let hr = d3d_ref.CreateDevice(
                D3DADAPTER_DEFAULT,
                D3DDEVTYPE_HAL,
                self.hwnd as _,
                D3DCREATE_HARDWARE_VERTEXPROCESSING,
                &mut self.present_params,
                &mut device,
            );

            if FAILED(hr) {
                eprintln!("Failed to create D3D9 device with hardware VP: 0x{:08X}", hr);
                eprintln!("Trying software vertex processing...");

                // Try software vertex processing
                let hr = d3d_ref.CreateDevice(
                    D3DADAPTER_DEFAULT,
                    D3DDEVTYPE_HAL,
                    self.hwnd as _,
                    D3DCREATE_SOFTWARE_VERTEXPROCESSING,
                    &mut self.present_params,
                    &mut device,
                );

                if FAILED(hr) {
                    eprintln!("Failed to create D3D9 device with software VP: 0x{:08X}", hr);
                    return false;
                }
            }

            if device.is_null() {
                eprintln!("Device pointer is null despite success HRESULT");
                return false;
            }

            self.device = Some(ComPtr::from_raw(device));
            true
        }
    }

    fn reset_device(&mut self) {
        // Drop and recreate renderer on device reset
        self.renderer = None;

        unsafe {
            let device_ref = self.device.as_ref().unwrap();
            let hr = device_ref.Reset(&mut self.present_params);
            if FAILED(hr) {
                eprintln!("Device reset failed: 0x{:08X}", hr);
                return;
            }
        }

        // Recreate renderer
        match unsafe { Renderer::new(&mut self.imgui, self.device.clone().unwrap()) } {
            Ok(renderer) => {
                self.renderer = Some(renderer);
            }
            Err(e) => {
                eprintln!("Failed to recreate ImGui renderer: {}", e);
            }
        }
    }

    pub fn start_render(&mut self) -> bool {
        unsafe {
            // Process Windows messages
            let mut msg: MSG = mem::zeroed();
            while PeekMessageA(&mut msg, ptr::null_mut(), 0, 0, PM_REMOVE) != 0 {
                TranslateMessage(&msg);
                DispatchMessageA(&msg);

                if msg.message == WM_QUIT {
                    return false;
                }
            }

            // Handle resize
            if self.resize_width != 0 && self.resize_height != 0 {
                self.present_params.BackBufferWidth = self.resize_width;
                self.present_params.BackBufferHeight = self.resize_height;
                self.resize_width = 0;
                self.resize_height = 0;
                self.reset_device();
            }

            // Poll mouse position relative to window
            let mut cursor_pos: POINT = mem::zeroed();
            GetCursorPos(&mut cursor_pos);
            ScreenToClient(self.hwnd, &mut cursor_pos);
            self.mouse_pos = [cursor_pos.x as f32, cursor_pos.y as f32];

            // High bit indicates if key is down
            self.mouse_buttons[0] = (GetAsyncKeyState(VK_LBUTTON) as u16 & 0x8000) != 0;
            self.mouse_buttons[1] = (GetAsyncKeyState(VK_RBUTTON) as u16 & 0x8000) != 0;
            self.mouse_buttons[2] = (GetAsyncKeyState(VK_MBUTTON) as u16 & 0x8000) != 0;
            self.mouse_buttons[3] = (GetAsyncKeyState(VK_XBUTTON1) as u16 & 0x8000) != 0;
            self.mouse_buttons[4] = (GetAsyncKeyState(VK_XBUTTON2) as u16 & 0x8000) != 0;

            // Update ImGui IO with mouse state
            let io = self.imgui.io_mut();
            io.mouse_pos = self.mouse_pos;
            io.mouse_down = self.mouse_buttons;

            // Update delta time
            let now = Instant::now();
            let delta = now - self.last_frame;
            io.delta_time = delta.as_secs_f32();
            self.last_frame = now;
        }

        true
    }

    pub fn render<F>(&mut self, f: F)
    where
        F: FnOnce(&Ui),
    {
        let ui = self.imgui.frame();
        f(&ui);

        // ui is dropped here and draw data is generated
        let draw_data = ui.render();

        unsafe {
            let device_ref = self.device.as_ref().unwrap();

            device_ref.SetRenderState(D3DRS_ZENABLE, FALSE as u32);
            device_ref.SetRenderState(D3DRS_ALPHABLENDENABLE, FALSE as u32);
            device_ref.SetRenderState(D3DRS_SCISSORTESTENABLE, FALSE as u32);

            device_ref.Clear(
                0,
                ptr::null_mut(),
                D3DCLEAR_TARGET | D3DCLEAR_ZBUFFER,
                0x00000000, // Transparent black
                1.0,
                0,
            );

            if SUCCEEDED(device_ref.BeginScene()) {
                if let Some(renderer) = &mut self.renderer {
                    let _ = renderer.render(draw_data);
                }
                device_ref.EndScene();
            }

            let hr = device_ref.Present(ptr::null_mut(), ptr::null_mut(), ptr::null_mut(), ptr::null_mut());

            // Handle device loss
            if hr == D3DERR_DEVICELOST as i32 {
                if device_ref.TestCooperativeLevel() == D3DERR_DEVICENOTRESET as i32 {
                    self.reset_device();
                }
            }
        }
    }

    pub fn handle_resize(&mut self, width: u32, height: u32) {
        self.resize_width = width;
        self.resize_height = height;
    }
}
