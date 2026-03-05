use color_eyre::eyre::OptionExt;
use wayper_windows::windows_host;
use windows::{
    Win32::{
        Foundation::{COLORREF, HINSTANCE, HWND, LPARAM, LRESULT, RECT, WPARAM},
        Graphics::Gdi::{
            BeginPaint, CreateSolidBrush, DeleteObject, EndPaint, FillRect, PAINTSTRUCT,
        },
        UI::WindowsAndMessaging::{
            self, CreateWindowExW, DefWindowProcW, DispatchMessageW, GetClientRect, GetMessageW,
            GetWindowLongPtrW, HWND_BOTTOM, LWA_ALPHA, MSG, PostQuitMessage, RegisterClassW,
            SW_SHOW, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SWP_SHOWWINDOW,
            SetLayeredWindowAttributes, SetWindowLongPtrW, SetWindowPos, ShowWindow,
            TranslateMessage, WINDOW_EX_STYLE, WINDOW_STYLE, WM_DESTROY, WM_PAINT, WNDCLASSW,
            WS_CHILD, WS_EX_LAYERED, WS_POPUP,
        },
    },
    core::PCWSTR,
};

const CLASS_NAME: PCWSTR = windows::core::w!("WayperRawDesktopChild");

fn main() -> color_eyre::Result<()> {
    color_eyre::install()?;

    let progman = windows_host::get_progman()?;
    let workerw = windows_host::spawn_workerw(progman)?;
    let shelldll =
        windows_host::find_shelldll_defview(progman).ok_or_eyre("SHELLDLL_DefView not found")?;

    let hwnd = unsafe { create_debug_window()? };
    unsafe { attach_to_desktop(hwnd, progman, shelldll, workerw)? };
    unsafe { run_message_loop()? };

    Ok(())
}

unsafe fn create_debug_window() -> color_eyre::Result<HWND> {
    unsafe {
        let wc = WNDCLASSW {
            lpfnWndProc: Some(wndproc),
            lpszClassName: CLASS_NAME,
            hInstance: HINSTANCE::default(),
            ..Default::default()
        };

        let atom = RegisterClassW(&wc);
        if atom == 0 {
            color_eyre::eyre::bail!("RegisterClassW failed");
        }

        let hwnd = CreateWindowExW(
            WINDOW_EX_STYLE(WS_EX_LAYERED.0),
            CLASS_NAME,
            windows::core::w!("Wayper Raw Desktop Child"),
            WINDOW_STYLE(WS_POPUP.0),
            0,
            0,
            1,
            1,
            None,
            None,
            Some(HINSTANCE::default()),
            None,
        )?;

        if hwnd.0.is_null() {
            color_eyre::eyre::bail!("CreateWindowExW failed");
        }

        Ok(hwnd)
    }
}

unsafe fn attach_to_desktop(
    hwnd: HWND,
    progman: HWND,
    shelldll: HWND,
    workerw: HWND,
) -> color_eyre::Result<()> {
    unsafe {
        let style = GetWindowLongPtrW(hwnd, WindowsAndMessaging::GWL_STYLE);
        SetWindowLongPtrW(
            hwnd,
            WindowsAndMessaging::GWL_STYLE,
            style | WS_CHILD.0 as isize,
        );

        let ex = GetWindowLongPtrW(hwnd, WindowsAndMessaging::GWL_EXSTYLE);
        SetWindowLongPtrW(
            hwnd,
            WindowsAndMessaging::GWL_EXSTYLE,
            ex | WS_EX_LAYERED.0 as isize,
        );
        SetLayeredWindowAttributes(hwnd, COLORREF(0), 255, LWA_ALPHA)?;

        WindowsAndMessaging::SetParent(hwnd, Some(progman))?;

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

        let _ = ShowWindow(hwnd, SW_SHOW);

        Ok(())
    }
}

unsafe fn run_message_loop() -> color_eyre::Result<()> {
    unsafe {
        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).into() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }

        Ok(())
    }
}

extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    unsafe {
        match msg {
            WM_PAINT => {
                let mut ps = PAINTSTRUCT::default();
                let hdc = BeginPaint(hwnd, &mut ps);
                let brush = CreateSolidBrush(COLORREF(0x0000FF));
                let mut rect = RECT::default();
                let _ = GetClientRect(hwnd, &mut rect);
                let _ = FillRect(hdc, &rect, brush);
                let _ = DeleteObject(brush.into());
                let _ = EndPaint(hwnd, &ps);
                LRESULT(0)
            }
            WM_DESTROY => {
                PostQuitMessage(0);
                LRESULT(0)
            }
            _ => DefWindowProcW(hwnd, msg, wparam, lparam),
        }
    }
}
