use std::{ffi::c_void, sync::Arc};

use color_eyre::eyre::OptionExt;
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use wayper_windows::windows_host;
use windows::Win32::{
    Foundation::{COLORREF, HWND, RECT},
    Graphics::Gdi::{CreateSolidBrush, DeleteObject, FillRect, GetDC, ReleaseDC},
    UI::WindowsAndMessaging::{
        self, GetClassNameW, GetClientRect, GetParent, GetWindowLongPtrW, HWND_BOTTOM,
        SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SWP_SHOWWINDOW, SetWindowPos,
    },
};
use winit::{
    application::ApplicationHandler,
    event::WindowEvent,
    event_loop::{ActiveEventLoop, ControlFlow, EventLoop},
    window::Window,
};

fn main() -> color_eyre::Result<()> {
    color_eyre::install()?;

    let event_loop = EventLoop::<()>::new()?;
    event_loop.set_control_flow(ControlFlow::Wait);
    let mut app = App { window: None };
    event_loop.run_app(&mut app)?;
    Ok(())
}

struct App {
    window: Option<Arc<Window>>,
}

impl ApplicationHandler<()> for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }

        let window = Arc::new(
            event_loop
                .create_window(
                    Window::default_attributes()
                        .with_title("Wayper Winit Desktop Child")
                        .with_decorations(false)
                        .with_resizable(false),
                )
                .expect("create winit window"),
        );

        let hwnd = hwnd_from_winit(&window).expect("extract hwnd");
        log_window_state("winit pre-attach", hwnd);

        let progman = windows_host::get_progman().expect("get progman");
        let workerw = windows_host::spawn_workerw(progman).expect("spawn/find workerw");
        let shelldll = windows_host::find_shelldll_defview(progman)
            .ok_or_eyre("missing SHELLDLL_DefView")
            .expect("find SHELLDLL_DefView");

        windows_host::reparent_window(progman,  window.clone()).expect("reparent");
        attach_geometry(hwnd, progman, shelldll, workerw).expect("position");

        log_window_state("winit post-attach", hwnd);
        debug_fill_window(hwnd).expect("fill window");

        self.window = Some(window);
    }

    fn window_event(
        &mut self,
        event_loop: &ActiveEventLoop,
        _window_id: winit::window::WindowId,
        event: WindowEvent,
    ) {
        if let WindowEvent::CloseRequested = event {
            event_loop.exit();
        }
    }
}

fn hwnd_from_winit(window: &Arc<Window>) -> color_eyre::Result<HWND> {
    let hwnd = if let RawWindowHandle::Win32(h) = window.window_handle()?.window_handle()?.as_raw()
    {
        HWND(h.hwnd.get() as *mut c_void)
    } else {
        color_eyre::eyre::bail!("No HWND available")
    };

    Ok(hwnd)
}

fn attach_geometry(
    hwnd: HWND,
    progman: HWND,
    shelldll: HWND,
    workerw: HWND,
) -> color_eyre::Result<()> {
    unsafe {
        let mut rect = RECT::default();
        GetClientRect(progman, &mut rect)?;
        let width = rect.right - rect.left;
        let height = rect.bottom - rect.top;

        SetWindowPos(
            hwnd,
            Some(shelldll),
            0,
            0,
            width,
            height,
            SWP_NOACTIVATE | SWP_SHOWWINDOW,
        )?;

        SetWindowPos(
            workerw,
            Some(HWND_BOTTOM),
            0,
            0,
            0,
            0,
            SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
        )?;
    }

    Ok(())
}

fn debug_fill_window(hwnd: HWND) -> color_eyre::Result<()> {
    unsafe {
        let mut rect = RECT::default();
        WindowsAndMessaging::GetClientRect(hwnd, &mut rect)?;

        let dc = GetDC(Some(hwnd));
        if dc.is_invalid() {
            color_eyre::eyre::bail!("GetDC failed for winit window");
        }

        let brush = CreateSolidBrush(COLORREF(0x00FFFF));
        if brush.is_invalid() {
            let _ = ReleaseDC(Some(hwnd), dc);
            color_eyre::eyre::bail!("CreateSolidBrush failed for winit window");
        }

        let _ = FillRect(dc, &rect, brush);
        let _ = DeleteObject(brush.into());
        let _ = ReleaseDC(Some(hwnd), dc);
    }

    Ok(())
}

fn log_window_state(label: &str, hwnd: HWND) {
    unsafe {
        let style = GetWindowLongPtrW(hwnd, WindowsAndMessaging::GWL_STYLE);
        let exstyle = GetWindowLongPtrW(hwnd, WindowsAndMessaging::GWL_EXSTYLE);
        let parent = GetParent(hwnd);

        let mut class_buf = vec![0u16; 256];
        let class_len = GetClassNameW(hwnd, &mut class_buf) as usize;
        let class_name = String::from_utf16_lossy(&class_buf[..class_len]);

        eprintln!(
            "{label}: hwnd={hwnd:?} parent={parent:?} class={class_name} style=0x{style:016X} exstyle=0x{exstyle:016X}"
        );
    }
}
