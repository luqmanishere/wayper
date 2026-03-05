//! Windows specific stuff.
//!
//! To summarize, we are doing:
//! 1. Ensure the desktop is in raised state by sending 0x052C to Progman
//! 2. Discover handles for Progman, SHELLDLL_DefView, WorkerW
//! 3. Create the rendering window with WS_EX_LAYERED and make it fully opaque
//! 4. Re-parent the window to Progman
//! 5. Insert it just below the icon window
//! 6. Push the WorkerW window behind our window
//!
//! This info is lifted from https://github.com/rocksdanister/lively/issues/2074#issuecomment-3017842549
//!
//! Credits whatever goes to them.

use std::{ffi::c_void, sync::Arc};

use color_eyre::eyre::OptionExt;
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use windows::Win32::{
    Foundation::{COLORREF, HWND, LPARAM, RECT, WPARAM},
    Graphics::Gdi::{CreateSolidBrush, DeleteObject, FillRect, GetDC, ReleaseDC},
    UI::WindowsAndMessaging::{
        self, GWL_EXSTYLE, GWL_STYLE, HWND_BOTTOM, LWA_ALPHA, SMTO_NORMAL, SW_HIDE, SW_SHOW,
        SWP_FRAMECHANGED, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SWP_SHOWWINDOW, WS_CAPTION,
        WS_CHILD, WS_EX_APPWINDOW, WS_EX_CLIENTEDGE, WS_EX_COMPOSITED, WS_EX_DLGMODALFRAME,
        WS_EX_LAYERED, WS_EX_STATICEDGE, WS_EX_TOOLWINDOW, WS_EX_WINDOWEDGE, WS_MAXIMIZEBOX,
        WS_MINIMIZEBOX, WS_POPUP, WS_SYSMENU, WS_THICKFRAME,
    },
};
use winit::platform::windows::WindowExtWindows;

/// Get the Windows Progman, which will help us spawn our workerw window
pub fn get_progman() -> color_eyre::Result<HWND> {
    let hwnd = unsafe {
        let pcstr = windows::core::w!("Progman");
        let window = windows::Win32::UI::WindowsAndMessaging::FindWindowW(pcstr, None)?;
        log::info!("found progman: {window:?}");
        window
    };

    Ok(hwnd)
}

pub fn find_shelldll_defview(progman: HWND) -> Option<HWND> {
    unsafe {
        match WindowsAndMessaging::FindWindowExW(
            Some(progman),
            None,
            windows::core::w!("SHELLDLL_DefView"),
            None,
        ) {
            Ok(res) => {
                log::info!("found SHELLDef_View: {res:?}");

                // sanity check
                let mut buffer: Vec<u16> = vec![0; 1000];

                let read_len = WindowsAndMessaging::GetClassNameW(res, buffer.as_mut_slice());
                let win_text = String::from_utf16_lossy(&buffer);
                log::debug!("read {read_len} chars, window class: {win_text}");

                return Some(res);
            }
            Err(_e) => None,
        }
    }
}

/// Finds the WorkerW window.
pub fn find_workerw(progman: HWND) -> Option<HWND> {
    unsafe {
        match WindowsAndMessaging::FindWindowExW(
            Some(progman),
            None,
            windows::core::w!("WorkerW"),
            None,
        ) {
            Ok(res) => {
                log::info!("found workerw: {res:?}");

                // sanity check
                let mut buffer: Vec<u16> = vec![0; 1000];

                let read_len = WindowsAndMessaging::GetClassNameW(res, buffer.as_mut_slice());
                let win_text = String::from_utf16_lossy(&buffer);
                log::debug!("read {read_len} chars, window class: {win_text}");

                return Some(res);
            }
            Err(_e) => None,
        }
    }
}

/// Spawn a WorkerW window from a Progman HWND
pub fn spawn_workerw(progman: HWND) -> color_eyre::Result<HWND> {
    unsafe {
        log::info!("sending codes to spawn workerw...");
        let mut _result = Default::default();
        let _ = windows::Win32::UI::WindowsAndMessaging::SendMessageTimeoutW(
            progman,
            0x052C,
            WPARAM(0xD),
            LPARAM(1),
            SMTO_NORMAL,
            1000,
            Some(&mut _result),
        );

        // find the workerw
        return find_workerw(progman).ok_or_eyre("WorkerW not found or spawned!");
    };
}

pub fn reparent_window(
    progman: HWND,
    window: Arc<winit::window::Window>,
) -> color_eyre::Result<()> {
    unsafe {
        let hwnd = if let RawWindowHandle::Win32(h) =
            window.window_handle_any_thread()?.window_handle()?.as_raw()
        {
            HWND(h.hwnd.get() as *mut c_void)
        } else {
            color_eyre::eyre::bail!("No HWND available")
        };

        let style = WindowsAndMessaging::GetWindowLongPtrW(hwnd, GWL_STYLE);
        let borderless_mask = WS_CAPTION.0 as isize
            | WS_THICKFRAME.0 as isize
            | WS_SYSMENU.0 as isize
            | WS_MAXIMIZEBOX.0 as isize
            | WS_MINIMIZEBOX.0 as isize
            | WS_POPUP.0 as isize;
        let _ = WindowsAndMessaging::ShowWindow(hwnd, SW_HIDE);
        let new_style = (style | WS_CHILD.0 as isize) & !borderless_mask;
        WindowsAndMessaging::SetWindowLongPtrW(hwnd, GWL_STYLE, new_style);

        let ex = WindowsAndMessaging::GetWindowLongPtrW(hwnd, GWL_EXSTYLE);
        let borderless_ex_mask = WS_EX_DLGMODALFRAME.0 as isize
            | WS_EX_COMPOSITED.0 as isize
            | WS_EX_WINDOWEDGE.0 as isize
            | WS_EX_CLIENTEDGE.0 as isize
            | WS_EX_STATICEDGE.0 as isize
            | WS_EX_TOOLWINDOW.0 as isize
            | WS_EX_APPWINDOW.0 as isize;
        WindowsAndMessaging::SetWindowLongPtrW(
            hwnd,
            GWL_EXSTYLE,
            (ex | WS_EX_LAYERED.0 as isize) & !borderless_ex_mask,
        );
        WindowsAndMessaging::SetLayeredWindowAttributes(hwnd, COLORREF(0), 255, LWA_ALPHA)?;

        WindowsAndMessaging::SetParent(hwnd, Some(progman))?;
        WindowsAndMessaging::SetWindowPos(
            hwnd,
            None,
            0,
            0,
            0,
            0,
            SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE | SWP_FRAMECHANGED,
        )?;
    }
    Ok(())
}

pub fn set_z_pos(
    shelldll: HWND,
    workerw: HWND,
    window: Arc<winit::window::Window>,
) -> color_eyre::Result<()> {
    unsafe {
        let hwnd = if let RawWindowHandle::Win32(h) =
            window.window_handle_any_thread()?.window_handle()?.as_raw()
        {
            HWND(h.hwnd.get() as *mut c_void)
        } else {
            color_eyre::eyre::bail!("No HWND available")
        };

        let parent = WindowsAndMessaging::GetParent(hwnd)?;
        let mut rect = RECT::default();
        WindowsAndMessaging::GetClientRect(parent, &mut rect)?;
        let width = rect.right - rect.left;
        let height = rect.bottom - rect.top;

        WindowsAndMessaging::SetWindowPos(
            hwnd,
            Some(shelldll),
            0,
            0,
            width,
            height,
            SWP_NOACTIVATE | SWP_SHOWWINDOW,
        )?;

        WindowsAndMessaging::SetWindowPos(
            workerw,
            Some(HWND_BOTTOM),
            0,
            0,
            0,
            0,
            SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
        )?;

        let last_state = WindowsAndMessaging::ShowWindow(hwnd, SW_SHOW);
        if last_state.0 != 0 {
            log::info!("window was visible");
        } else {
            log::info!("window was not visible")
        }

        debug_fill_window(hwnd)?;
    }
    Ok(())
}

pub fn debug_fill_window(hwnd: HWND) -> color_eyre::Result<()> {
    unsafe {
        let mut rect = RECT::default();
        WindowsAndMessaging::GetClientRect(hwnd, &mut rect)?;

        let dc = GetDC(Some(hwnd));
        if dc.is_invalid() {
            color_eyre::eyre::bail!("GetDC failed for wallpaper window");
        }

        let brush = CreateSolidBrush(COLORREF(0x000000));
        if brush.is_invalid() {
            let _ = ReleaseDC(Some(hwnd), dc);
            color_eyre::eyre::bail!("CreateSolidBrush failed for wallpaper window");
        }

        let _ = FillRect(dc, &rect, brush);
        let _ = DeleteObject(brush.into());
        let _ = ReleaseDC(Some(hwnd), dc);
    }

    Ok(())
}
