//! Plugin window management
//!
//! This module provides platform-specific window creation and management
//! for VST3 plugin GUIs.

use crate::error::{Error, Result};
use crate::plugin::Plugin;
use crate::process_isolation::{EditorHostAction, EditorHostState};
use std::sync::{Arc, Mutex};

#[cfg(target_os = "macos")]
use objc2::{rc::Retained, MainThreadMarker, MainThreadOnly};
#[cfg(target_os = "macos")]
use objc2_app_kit::{NSApplication, NSBackingStoreType, NSView, NSWindow, NSWindowStyleMask};
#[cfg(target_os = "macos")]
use objc2_foundation::{NSPoint, NSRect, NSSize, NSString};

#[cfg(target_os = "windows")]
use winapi::{
    shared::minwindef::{LPARAM, LRESULT, UINT, WPARAM},
    shared::windef::{HDC, HWND, POINT, RECT},
    um::libloaderapi::GetModuleHandleW,
    um::wingdi::{
        GetStockObject, LineTo, MoveToEx, SelectObject, SetBkMode, SetDCBrushColor,
        SetDCPenColor, SetTextColor, DC_BRUSH, DC_PEN, DEFAULT_GUI_FONT, TRANSPARENT,
    },
    um::winuser::{
        BeginPaint, CreateWindowExW, DefWindowProcW, DestroyWindow, DrawTextW, EndPaint, FillRect,
        GetClientRect, GetDpiForWindow, GetWindowLongPtrW, GetWindowTextW, InvalidateRect,
        LoadCursorW, MoveWindow, RegisterClassExW, ScreenToClient,
        SetWindowLongPtrW, SetWindowPos, ShowWindow, TrackMouseEvent, UpdateWindow, CS_DROPSHADOW,
        CW_USEDEFAULT, DT_CENTER, DT_END_ELLIPSIS, DT_NOPREFIX, DT_SINGLELINE, DT_VCENTER,
        GWLP_USERDATA, HTCAPTION, HTCLIENT, IDC_ARROW, PAINTSTRUCT, SWP_NOACTIVATE, SWP_NOMOVE,
        SWP_NOZORDER, SW_MINIMIZE, SW_SHOW, TME_LEAVE, TRACKMOUSEEVENT, WM_CLOSE, WM_DPICHANGED,
        WM_ENTERSIZEMOVE, WM_ERASEBKGND, WM_EXITSIZEMOVE, WM_LBUTTONDOWN, WM_LBUTTONUP,
        WM_MOUSELEAVE, WM_MOUSEMOVE, WM_NCHITTEST, WM_PAINT, WM_SIZE, WNDCLASSEXW, WS_CHILD,
        WS_CLIPCHILDREN, WS_CLIPSIBLINGS, WS_EX_APPWINDOW, WS_MINIMIZEBOX, WS_POPUP, WS_VISIBLE,
    },
};

#[cfg(target_os = "windows")]
const HOST_CHROME_TOP_HEIGHT_DIP: i32 = 34;
#[cfg(target_os = "windows")]
const HOST_CHROME_ACTION_HEIGHT_DIP: i32 = 30;
#[cfg(target_os = "windows")]
const HOST_CHROME_HEIGHT_DIP: i32 = HOST_CHROME_TOP_HEIGHT_DIP + HOST_CHROME_ACTION_HEIGHT_DIP;
#[cfg(target_os = "windows")]
const HOST_CHROME_BUTTON_WIDTH_DIP: i32 = 42;
#[cfg(target_os = "windows")]
const HOST_CHROME_BYPASS_WIDTH_DIP: i32 = 78;
#[cfg(target_os = "windows")]
const HOST_CHROME_SAVE_WIDTH_DIP: i32 = 54;
#[cfg(target_os = "windows")]
const HOST_CHROME_LOAD_WIDTH_DIP: i32 = 68;
#[cfg(target_os = "windows")]
const HOST_CHROME_SIDECHAIN_WIDTH_DIP: i32 = 82;
#[cfg(target_os = "windows")]
const HOST_CHROME_AUTOMATION_LABEL_WIDTH_DIP: i32 = 72;
#[cfg(target_os = "windows")]
const HOST_CHROME_AUTOMATION_BUTTON_WIDTH_DIP: i32 = 24;

#[cfg(target_os = "windows")]
fn scale_dip(value: i32, dpi: u32) -> i32 {
    ((i64::from(value) * i64::from(dpi.max(96)) + 48) / 96) as i32
}

#[cfg(target_os = "windows")]
fn host_chrome_height(hwnd: HWND) -> i32 {
    scale_dip(HOST_CHROME_HEIGHT_DIP, unsafe { GetDpiForWindow(hwnd) })
}

#[cfg(target_os = "windows")]
fn color(r: u8, g: u8, b: u8) -> u32 {
    u32::from(r) | (u32::from(g) << 8) | (u32::from(b) << 16)
}

#[cfg(target_os = "windows")]
#[derive(Clone, Copy, PartialEq, Eq)]
enum HostChromeButton {
    Power,
    Pin,
    Minimize,
    Close,
    Bypass,
    Save,
    Load,
    Sidechain,
    AutomationOff,
    AutomationWrite,
    AutomationRead,
    AutomationLatch,
}

#[cfg(target_os = "windows")]
#[derive(Default)]
struct HostChromeState {
    actions: Vec<EditorHostAction>,
    pinned: bool,
    bypassed: bool,
    has_sidechain: bool,
    automation: u8,
    hovered: Option<HostChromeButton>,
    tracking_mouse_leave: bool,
    interactive_move: bool,
}

#[cfg(target_os = "windows")]
fn host_chrome_states() -> &'static Mutex<std::collections::HashMap<usize, HostChromeState>> {
    static STATES: std::sync::OnceLock<
        Mutex<std::collections::HashMap<usize, HostChromeState>>,
    > = std::sync::OnceLock::new();
    STATES.get_or_init(Mutex::default)
}

#[cfg(target_os = "windows")]
#[derive(Clone, Copy)]
struct HostChromeLayout {
    width: i32,
    top_height: i32,
    chrome_height: i32,
    dpi: u32,
    has_sidechain: bool,
}

#[cfg(target_os = "windows")]
impl HostChromeLayout {
    fn new(width: i32, chrome_height: i32, dpi: u32, has_sidechain: bool) -> Self {
        Self {
            width: width.max(0),
            top_height: scale_dip(HOST_CHROME_TOP_HEIGHT_DIP, dpi).min(chrome_height.max(0)),
            chrome_height: chrome_height.max(0),
            dpi,
            has_sidechain,
        }
    }

    fn automation_start(&self) -> i32 {
        (self.width
            - scale_dip(HOST_CHROME_AUTOMATION_BUTTON_WIDTH_DIP, self.dpi) * 4)
            .max(0)
    }

    fn left_action_end(&self) -> i32 {
        let widths = [
            HOST_CHROME_BYPASS_WIDTH_DIP,
            HOST_CHROME_SAVE_WIDTH_DIP,
            HOST_CHROME_LOAD_WIDTH_DIP,
        ];
        let mut right: i32 = widths
            .into_iter()
            .map(|width| scale_dip(width, self.dpi))
            .sum();
        if self.has_sidechain {
            let with_sidechain = right + scale_dip(HOST_CHROME_SIDECHAIN_WIDTH_DIP, self.dpi);
            if with_sidechain <= self.automation_start() {
                right = with_sidechain;
            }
        }
        right.min(self.automation_start())
    }

    fn button_rect(&self, button: HostChromeButton) -> Option<RECT> {
        let top_button_width = scale_dip(HOST_CHROME_BUTTON_WIDTH_DIP, self.dpi);
        let top_rect = |left: i32, right: i32| RECT {
            left,
            top: 0,
            right,
            bottom: self.top_height,
        };
        match button {
            HostChromeButton::Power => Some(top_rect(0, top_button_width)),
            HostChromeButton::Pin => Some(top_rect(
                self.width - top_button_width * 3,
                self.width - top_button_width * 2,
            )),
            HostChromeButton::Minimize => Some(top_rect(
                self.width - top_button_width * 2,
                self.width - top_button_width,
            )),
            HostChromeButton::Close => Some(top_rect(
                self.width - top_button_width,
                self.width,
            )),
            HostChromeButton::AutomationOff
            | HostChromeButton::AutomationWrite
            | HostChromeButton::AutomationRead
            | HostChromeButton::AutomationLatch => {
                let tiny = scale_dip(HOST_CHROME_AUTOMATION_BUTTON_WIDTH_DIP, self.dpi);
                let index = match button {
                    HostChromeButton::AutomationOff => 0,
                    HostChromeButton::AutomationWrite => 1,
                    HostChromeButton::AutomationRead => 2,
                    HostChromeButton::AutomationLatch => 3,
                    _ => unreachable!(),
                };
                let left = self.automation_start() + tiny * index;
                Some(RECT {
                    left,
                    top: self.top_height,
                    right: left + tiny,
                    bottom: self.chrome_height,
                })
            }
            HostChromeButton::Bypass
            | HostChromeButton::Save
            | HostChromeButton::Load
            | HostChromeButton::Sidechain => {
                let definitions = [
                    (HostChromeButton::Bypass, HOST_CHROME_BYPASS_WIDTH_DIP),
                    (HostChromeButton::Save, HOST_CHROME_SAVE_WIDTH_DIP),
                    (HostChromeButton::Load, HOST_CHROME_LOAD_WIDTH_DIP),
                    (HostChromeButton::Sidechain, HOST_CHROME_SIDECHAIN_WIDTH_DIP),
                ];
                let mut left = 0;
                for (candidate, width_dip) in definitions {
                    if candidate == HostChromeButton::Sidechain && !self.has_sidechain {
                        continue;
                    }
                    let right = left + scale_dip(width_dip, self.dpi);
                    if right > self.automation_start() {
                        return None;
                    }
                    if candidate == button {
                        return Some(RECT {
                            left,
                            top: self.top_height,
                            right,
                            bottom: self.chrome_height,
                        });
                    }
                    left = right;
                }
                None
            }
        }
    }

    fn automation_label_rect(&self) -> Option<RECT> {
        let right = self.automation_start();
        let left = right - scale_dip(HOST_CHROME_AUTOMATION_LABEL_WIDTH_DIP, self.dpi);
        (left >= self.left_action_end() + scale_dip(4, self.dpi)).then_some(RECT {
            left,
            top: self.top_height,
            right,
            bottom: self.chrome_height,
        })
    }
}

#[cfg(target_os = "windows")]
fn host_chrome_layout(hwnd: HWND, has_sidechain: bool) -> Option<HostChromeLayout> {
    let mut client: RECT = unsafe { std::mem::zeroed() };
    if unsafe { GetClientRect(hwnd, &mut client) } == 0 {
        return None;
    }
    let dpi = unsafe { GetDpiForWindow(hwnd) };
    Some(HostChromeLayout::new(
        client.right.max(0),
        host_chrome_height(hwnd).min(client.bottom.max(0)),
        dpi,
        has_sidechain,
    ))
}

#[cfg(target_os = "windows")]
fn host_chrome_button_at(hwnd: HWND, x: i32, y: i32) -> Option<HostChromeButton> {
    let has_sidechain = host_chrome_states()
        .lock()
        .ok()
        .and_then(|states| states.get(&(hwnd as usize)).map(|state| state.has_sidechain))
        .unwrap_or(false);
    let layout = host_chrome_layout(hwnd, has_sidechain)?;
    if x < 0 || x >= layout.width || y < 0 || y >= layout.chrome_height {
        return None;
    }
    const BUTTONS: [HostChromeButton; 12] = [
        HostChromeButton::Power,
        HostChromeButton::Pin,
        HostChromeButton::Minimize,
        HostChromeButton::Close,
        HostChromeButton::Bypass,
        HostChromeButton::Save,
        HostChromeButton::Load,
        HostChromeButton::Sidechain,
        HostChromeButton::AutomationOff,
        HostChromeButton::AutomationWrite,
        HostChromeButton::AutomationRead,
        HostChromeButton::AutomationLatch,
    ];
    BUTTONS.into_iter().find(|button| {
        layout.button_rect(*button).is_some_and(|rect| {
            x >= rect.left && x < rect.right && y >= rect.top && y < rect.bottom
        })
    })
}

#[cfg(target_os = "windows")]
unsafe fn invalidate_host_chrome(hwnd: HWND) {
    let mut client: RECT = std::mem::zeroed();
    if GetClientRect(hwnd, &mut client) != 0 {
        client.bottom = host_chrome_height(hwnd).min(client.bottom.max(0));
        InvalidateRect(hwnd, &client, 0);
    }
}

#[cfg(target_os = "windows")]
unsafe fn invalidate_host_button(hwnd: HWND, button: HostChromeButton) {
    let has_sidechain = host_chrome_states()
        .lock()
        .ok()
        .and_then(|states| states.get(&(hwnd as usize)).map(|state| state.has_sidechain))
        .unwrap_or(false);
    if let Some(rect) = host_chrome_layout(hwnd, has_sidechain)
        .and_then(|layout| layout.button_rect(button))
    {
        InvalidateRect(hwnd, &rect, 0);
    }
}

#[cfg(target_os = "windows")]
fn point_from_lparam(lparam: LPARAM) -> (i32, i32) {
    ((lparam as i16) as i32, ((lparam >> 16) as i16) as i32)
}

#[cfg(target_os = "windows")]
unsafe fn fill_rect(hdc: HDC, rect: &RECT, fill: u32) {
    let brush = GetStockObject(DC_BRUSH as i32);
    SetDCBrushColor(hdc, fill);
    FillRect(hdc, rect, brush.cast());
}

#[cfg(target_os = "windows")]
unsafe fn draw_chrome_label(hdc: HDC, rect: &RECT, label: &str, text_color: u32) {
    let mut wide = [0_u16; 32];
    let mut length = 0;
    for unit in label.encode_utf16().take(wide.len()) {
        wide[length] = unit;
        length += 1;
    }
    let mut rect = *rect;
    SetTextColor(hdc, text_color);
    DrawTextW(
        hdc,
        wide.as_ptr(),
        length as i32,
        &mut rect,
        DT_SINGLELINE | DT_VCENTER | DT_CENTER | DT_NOPREFIX,
    );
}

/// Paint the host-owned toolbar. It is intentionally GDI-only: no WebView,
/// compositor or second UI thread is introduced into the plug-in process.
#[cfg(target_os = "windows")]
unsafe fn paint_host_chrome(hwnd: HWND) {
    let mut paint: PAINTSTRUCT = std::mem::zeroed();
    let hdc = BeginPaint(hwnd, &mut paint);
    if hdc.is_null() {
        return;
    }
    let mut client: RECT = std::mem::zeroed();
    GetClientRect(hwnd, &mut client);
    let (pinned, bypassed, has_sidechain, automation, hovered) = host_chrome_states()
        .lock()
        .ok()
        .and_then(|states| {
            states.get(&(hwnd as usize)).map(|state| {
                (
                    state.pinned,
                    state.bypassed,
                    state.has_sidechain,
                    state.automation,
                    state.hovered,
                )
            })
        })
        .unwrap_or((false, false, false, 0, None));
    let dpi = GetDpiForWindow(hwnd);
    let chrome_height = host_chrome_height(hwnd).min(client.bottom.max(0));
    let layout = HostChromeLayout::new(client.right, chrome_height, dpi, has_sidechain);
    let toolbar = RECT {
        left: 0,
        top: 0,
        right: client.right,
        bottom: chrome_height,
    };
    // The plug-in child owns every pixel below this toolbar. Painting the
    // entire parent client on hover forced DWM to reconsider a potentially
    // 4K vendor surface for a one-button color change.
    fill_rect(hdc, &toolbar, color(17, 29, 39));
    let action_row = RECT {
        left: 0,
        top: layout.top_height,
        right: client.right,
        bottom: chrome_height,
    };
    fill_rect(hdc, &action_row, color(13, 23, 31));
    let accent = RECT {
        left: 0,
        top: chrome_height.saturating_sub(scale_dip(1, dpi)),
        right: client.right,
        bottom: chrome_height,
    };
    fill_rect(hdc, &accent, color(63, 113, 137));

    let power_rect = layout.button_rect(HostChromeButton::Power).unwrap();
    let pin_rect = layout.button_rect(HostChromeButton::Pin).unwrap();
    let minimize_rect = layout.button_rect(HostChromeButton::Minimize).unwrap();
    let close_rect = layout.button_rect(HostChromeButton::Close).unwrap();
    fill_rect(
        hdc,
        &power_rect,
        if bypassed {
            color(29, 41, 49)
        } else if hovered == Some(HostChromeButton::Power) {
            color(23, 83, 99)
        } else {
            color(19, 68, 82)
        },
    );
    fill_rect(
        hdc,
        &pin_rect,
        if pinned {
            color(77, 64, 25)
        } else if hovered == Some(HostChromeButton::Pin) {
            color(27, 52, 65)
        } else {
            color(17, 29, 39)
        },
    );
    if hovered == Some(HostChromeButton::Minimize) {
        fill_rect(hdc, &minimize_rect, color(27, 52, 65));
    }
    fill_rect(
        hdc,
        &close_rect,
        if hovered == Some(HostChromeButton::Close) {
            color(151, 52, 63)
        } else {
            color(31, 44, 54)
        },
    );

    let icon_half = scale_dip(5, dpi);
    let pen = GetStockObject(DC_PEN as i32);
    SetDCPenColor(hdc, color(211, 229, 237));
    let previous_pen = SelectObject(hdc, pen);
    let min_x = (minimize_rect.left + minimize_rect.right) / 2;
    let center_y = layout.top_height / 2;
    MoveToEx(
        hdc,
        min_x - icon_half,
        center_y + scale_dip(3, dpi),
        std::ptr::null_mut(),
    );
    LineTo(hdc, min_x + icon_half + 1, center_y + scale_dip(3, dpi));
    let close_x = (close_rect.left + close_rect.right) / 2;
    MoveToEx(
        hdc,
        close_x - icon_half,
        center_y - icon_half,
        std::ptr::null_mut(),
    );
    LineTo(hdc, close_x + icon_half + 1, center_y + icon_half + 1);
    MoveToEx(
        hdc,
        close_x + icon_half,
        center_y - icon_half,
        std::ptr::null_mut(),
    );
    LineTo(hdc, close_x - icon_half - 1, center_y + icon_half + 1);
    SelectObject(hdc, previous_pen);

    let mut title = [0_u16; 256];
    let read = GetWindowTextW(
        hwnd,
        title.as_mut_ptr(),
        title.len() as i32,
    );
    let mut title_rect = RECT {
        left: power_rect.right + scale_dip(10, dpi),
        top: 0,
        right: (pin_rect.left - scale_dip(8, dpi)).max(0),
        bottom: layout.top_height,
    };
    SetBkMode(hdc, TRANSPARENT as i32);
    SetTextColor(hdc, color(216, 231, 240));
    let font = GetStockObject(DEFAULT_GUI_FONT as i32);
    let previous_font = SelectObject(hdc, font);
    if read > 0 {
        DrawTextW(
            hdc,
            title.as_ptr(),
            read,
            &mut title_rect,
            DT_SINGLELINE | DT_VCENTER | DT_END_ELLIPSIS | DT_NOPREFIX,
        );
    }

    draw_chrome_label(
        hdc,
        &power_rect,
        "전원",
        if bypassed {
            color(105, 125, 135)
        } else {
            color(99, 221, 245)
        },
    );
    draw_chrome_label(
        hdc,
        &pin_rect,
        "고정",
        if pinned {
            color(242, 200, 91)
        } else {
            color(126, 151, 165)
        },
    );

    for (button, label, active_color) in [
        (
            HostChromeButton::Bypass,
            "바이패스",
            if bypassed {
                color(197, 92, 91)
            } else {
                color(103, 139, 155)
            },
        ),
        (HostChromeButton::Save, "저장", color(103, 139, 155)),
        (HostChromeButton::Load, "불러오기", color(103, 139, 155)),
        (
            HostChromeButton::Sidechain,
            "사이드체인",
            color(103, 139, 155),
        ),
    ] {
        let Some(rect) = layout.button_rect(button) else {
            continue;
        };
        fill_rect(
            hdc,
            &rect,
            if hovered == Some(button) {
                color(29, 49, 61)
            } else {
                color(13, 23, 31)
            },
        );
        draw_chrome_label(hdc, &rect, label, active_color);
    }

    if let Some(label_rect) = layout.automation_label_rect() {
        draw_chrome_label(hdc, &label_rect, "오토메이션", color(93, 121, 135));
    }
    for (button, active_color) in [
        (HostChromeButton::AutomationOff, color(138, 148, 155)),
        (HostChromeButton::AutomationWrite, color(240, 82, 88)),
        (HostChromeButton::AutomationRead, color(76, 201, 138)),
        (HostChromeButton::AutomationLatch, color(233, 189, 75)),
    ] {
        let rect = layout.button_rect(button).unwrap();
        let active = match button {
            HostChromeButton::AutomationOff => automation == 0,
            HostChromeButton::AutomationWrite => automation == 1,
            HostChromeButton::AutomationRead => automation == 2,
            HostChromeButton::AutomationLatch => automation == 3,
            _ => false,
        };
        fill_rect(
            hdc,
            &rect,
            if hovered == Some(button) || active {
                color(29, 49, 61)
            } else {
                color(13, 23, 31)
            },
        );
        let inset = scale_dip(if active { 7 } else { 9 }, dpi);
        let swatch = RECT {
            left: rect.left + inset,
            top: rect.top + inset,
            right: rect.right - inset,
            bottom: rect.bottom - inset,
        };
        fill_rect(hdc, &swatch, active_color);
        if active {
            let underline = RECT {
                left: rect.left + scale_dip(4, dpi),
                top: rect.bottom - scale_dip(2, dpi),
                right: rect.right - scale_dip(4, dpi),
                bottom: rect.bottom - scale_dip(1, dpi),
            };
            fill_rect(hdc, &underline, active_color);
        }
    }
    SelectObject(hdc, previous_font);
    EndPaint(hwnd, &paint);
}

#[cfg(any(test, target_os = "windows"))]
fn dpi_scale_factor(dpi: u32) -> Option<f32> {
    (dpi > 0).then_some(dpi as f32 / 96.0)
}

/// Editor windows whose window procedure has seen `WM_CLOSE`, keyed by `HWND` (as `usize`,
/// since `HWND` is a raw pointer and not `Send`).
///
/// `DefWindowProcW` answers `WM_CLOSE` by destroying the window, which would leave the plugin's
/// `IPlugView` attached to a freed `HWND`. [`plugin_window_proc`] records the request here
/// instead, so the host can tear the editor down in the right order.
#[cfg(target_os = "windows")]
fn close_requests() -> &'static Mutex<std::collections::HashSet<usize>> {
    static REQUESTS: std::sync::OnceLock<Mutex<std::collections::HashSet<usize>>> =
        std::sync::OnceLock::new();
    REQUESTS.get_or_init(Mutex::default)
}

/// Latest pending DPI for each editor window. The window procedure cannot call into the plugin:
/// `setContentScaleFactor` may synchronously enter arbitrary plugin/UI code, while callers often
/// hold the plugin mutex when Windows dispatches a message. The procedure records only the
/// newest DPI and [`PluginWindow::service_platform_events`] applies it after the callback returns.
#[cfg(target_os = "windows")]
fn dpi_changes() -> &'static Mutex<std::collections::HashMap<usize, u32>> {
    static CHANGES: std::sync::OnceLock<Mutex<std::collections::HashMap<usize, u32>>> =
        std::sync::OnceLock::new();
    CHANGES.get_or_init(Mutex::default)
}

#[cfg(target_os = "windows")]
fn dpi_from_wparam(wparam: WPARAM) -> Option<u32> {
    // WM_DPICHANGED packs the new X DPI into LOWORD and Y DPI into HIWORD. VST3 has one content
    // scale, and Windows normally reports both equally, so use X as Microsoft recommends.
    let dpi = (wparam & 0xffff) as u32;
    (dpi > 0).then_some(dpi)
}

/// Window procedure for plugin editor windows: record control-plane work and defer plugin calls
/// until after Windows returns from the callback.
///
/// # Safety
///
/// Called by the OS with the arguments of a window procedure; all it does with them is forward
/// them to `DefWindowProcW`.
#[cfg(target_os = "windows")]
unsafe extern "system" fn plugin_window_proc(
    hwnd: HWND,
    msg: UINT,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    if msg == WM_NCHITTEST {
        let (x, y) = point_from_lparam(lparam);
        let mut point = POINT { x, y };
        if ScreenToClient(hwnd, &mut point) != 0
            && point.y >= 0
            && point.y < host_chrome_height(hwnd)
        {
            return if host_chrome_button_at(hwnd, point.x, point.y).is_some() {
                HTCLIENT as LRESULT
            } else {
                HTCAPTION as LRESULT
            };
        }
        return HTCLIENT as LRESULT;
    }
    if msg == WM_LBUTTONDOWN {
        let (x, y) = point_from_lparam(lparam);
        if host_chrome_button_at(hwnd, x, y).is_some() {
            return 0;
        }
    }
    if msg == WM_LBUTTONUP {
        let (x, y) = point_from_lparam(lparam);
        match host_chrome_button_at(hwnd, x, y) {
            Some(button @ (HostChromeButton::Power
            | HostChromeButton::Pin
            | HostChromeButton::Bypass
            | HostChromeButton::Save
            | HostChromeButton::Load
            | HostChromeButton::Sidechain
            | HostChromeButton::AutomationOff
            | HostChromeButton::AutomationWrite
            | HostChromeButton::AutomationRead
            | HostChromeButton::AutomationLatch)) => {
                if let Ok(mut states) = host_chrome_states().lock() {
                    let state = states.entry(hwnd as usize).or_default();
                    let action = match button {
                        HostChromeButton::Power => {
                            state.bypassed = !state.bypassed;
                            EditorHostAction::TogglePower
                        }
                        HostChromeButton::Pin => {
                            state.pinned = !state.pinned;
                            EditorHostAction::TogglePin
                        }
                        HostChromeButton::Bypass => {
                            state.bypassed = !state.bypassed;
                            EditorHostAction::ToggleBypass
                        }
                        HostChromeButton::Save => EditorHostAction::SavePreset,
                        HostChromeButton::Load => EditorHostAction::LoadPreset,
                        HostChromeButton::Sidechain => EditorHostAction::ShowSidechain,
                        HostChromeButton::AutomationOff => {
                            state.automation = 0;
                            EditorHostAction::AutomationOff
                        }
                        HostChromeButton::AutomationWrite => {
                            state.automation = 1;
                            EditorHostAction::AutomationWrite
                        }
                        HostChromeButton::AutomationRead => {
                            state.automation = 2;
                            EditorHostAction::AutomationRead
                        }
                        HostChromeButton::AutomationLatch => {
                            state.automation = 3;
                            EditorHostAction::AutomationLatch
                        }
                        _ => unreachable!(),
                    };
                    // UI gestures are human-rate; the cap prevents an unattended helper from
                    // growing if the main host disappears without closing the editor.
                    if state.actions.len() < 64 {
                        state.actions.push(action);
                    }
                }
                invalidate_host_chrome(hwnd);
                return 0;
            }
            Some(HostChromeButton::Minimize) => {
                ShowWindow(hwnd, SW_MINIMIZE);
                return 0;
            }
            Some(HostChromeButton::Close) => {
                if let Ok(mut requests) = close_requests().lock() {
                    requests.insert(hwnd as usize);
                }
                return 0;
            }
            None => {}
        }
    }
    if msg == WM_MOUSEMOVE {
        let (x, y) = point_from_lparam(lparam);
        let hovered = host_chrome_button_at(hwnd, x, y);
        let (previous, changed, register_leave) = host_chrome_states()
            .lock()
            .ok()
            .map(|mut states| {
                let state = states.entry(hwnd as usize).or_default();
                if state.interactive_move {
                    return (state.hovered, false, false);
                }
                let previous = state.hovered;
                let changed = previous != hovered;
                state.hovered = hovered;
                let register_leave = !state.tracking_mouse_leave;
                state.tracking_mouse_leave = true;
                (previous, changed, register_leave)
            })
            .unwrap_or((None, false, false));
        if register_leave {
            let mut tracking = TRACKMOUSEEVENT {
                cbSize: std::mem::size_of::<TRACKMOUSEEVENT>() as u32,
                dwFlags: TME_LEAVE,
                hwndTrack: hwnd,
                dwHoverTime: 0,
            };
            TrackMouseEvent(&mut tracking);
        }
        // Repaint only when the hover target changes, and only the two small
        // button rectangles involved. A title-bar drag no longer invalidates
        // the plug-in-sized parent surface for every pointer pixel.
        if changed {
            if let Some(button) = previous {
                invalidate_host_button(hwnd, button);
            }
            if let Some(button) = hovered {
                invalidate_host_button(hwnd, button);
            }
        }
        return 0;
    }
    if msg == WM_MOUSELEAVE {
        let previous = host_chrome_states().lock().ok().and_then(|mut states| {
            let state = states.get_mut(&(hwnd as usize))?;
            state.tracking_mouse_leave = false;
            state.hovered.take()
        });
        if let Some(button) = previous {
            invalidate_host_button(hwnd, button);
        }
        return 0;
    }
    if msg == WM_ENTERSIZEMOVE {
        let previous = host_chrome_states().lock().ok().and_then(|mut states| {
            let state = states.get_mut(&(hwnd as usize))?;
            state.interactive_move = true;
            state.tracking_mouse_leave = false;
            state.hovered.take()
        });
        if let Some(button) = previous {
            invalidate_host_button(hwnd, button);
        }
        return 0;
    }
    if msg == WM_EXITSIZEMOVE {
        if let Ok(mut states) = host_chrome_states().lock() {
            if let Some(state) = states.get_mut(&(hwnd as usize)) {
                state.interactive_move = false;
                state.tracking_mouse_leave = false;
            }
        }
        return 0;
    }
    if msg == WM_SIZE {
        let container = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as HWND;
        if !container.is_null() {
            let mut client: RECT = std::mem::zeroed();
            if GetClientRect(hwnd, &mut client) != 0 {
                let top = host_chrome_height(hwnd);
                MoveWindow(
                    container,
                    0,
                    top,
                    client.right.max(1),
                    (client.bottom - top).max(1),
                    1,
                );
            }
        }
        invalidate_host_chrome(hwnd);
        return 0;
    }
    if msg == WM_PAINT {
        paint_host_chrome(hwnd);
        return 0;
    }
    if msg == WM_ERASEBKGND {
        return 1;
    }
    if msg == WM_CLOSE {
        if let Ok(mut requests) = close_requests().lock() {
            requests.insert(hwnd as usize);
        }
        return 0;
    }
    if msg == WM_DPICHANGED {
        // The suggested rectangle is valid only for this callback and must be applied
        // synchronously. Do that before taking any host mutex: SetWindowPos may dispatch more
        // window messages reentrantly.
        if let Some(suggested) = (lparam as *const RECT).as_ref() {
            SetWindowPos(
                hwnd,
                std::ptr::null_mut(),
                suggested.left,
                suggested.top,
                suggested.right - suggested.left,
                suggested.bottom - suggested.top,
                SWP_NOZORDER | SWP_NOACTIVATE,
            );
        }
        if let Some(dpi) = dpi_from_wparam(wparam) {
            if let Ok(mut changes) = dpi_changes().lock() {
                changes.insert(hwnd as usize, dpi);
            }
        }
        return 0;
    }
    DefWindowProcW(hwnd, msg, wparam, lparam)
}

/// An X11 window (connection + window id) backing a plugin editor on Linux.
///
/// Ported from the khremeviuc1004 fork's XCB implementation.
#[cfg(target_os = "linux")]
struct XcbWindowState {
    connection: xcb::Connection,
    window: xcb::x::Window,
}

/// A plugin window that manages the native window and plugin editor lifecycle
pub struct PluginWindow {
    plugin: Arc<Mutex<Plugin>>,
    #[cfg(target_os = "macos")]
    native_window: Option<Retained<NSWindow>>,
    /// The view the plugin attached its editor into. Kept so a plugin-initiated resize can
    /// grow the container along with the window.
    #[cfg(target_os = "macos")]
    container_view: Option<Retained<NSView>>,
    #[cfg(target_os = "windows")]
    native_window: Option<HWND>,
    /// Same-process child HWND handed to `IPlugView::attached`. Keeping the
    /// editor below a host-owned toolbar avoids painting into vendor pixels.
    #[cfg(target_os = "windows")]
    editor_container: Option<HWND>,
    #[cfg(target_os = "linux")]
    native_window: Option<XcbWindowState>,
    #[cfg(target_os = "android")]
    native_window: Option<()>,
}

impl PluginWindow {
    /// Create a new plugin window for the given plugin
    pub fn new(plugin: Arc<Mutex<Plugin>>) -> Self {
        Self {
            plugin,
            #[cfg(any(
                target_os = "macos",
                target_os = "windows",
                target_os = "linux",
                target_os = "android"
            ))]
            native_window: None,
            #[cfg(target_os = "windows")]
            editor_container: None,
            #[cfg(target_os = "macos")]
            container_view: None,
        }
    }

    /// Open the plugin window
    pub fn open(&mut self) -> Result<()> {
        // Check if plugin has editor
        let has_editor = self
            .plugin
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .has_editor();
        if !has_editor {
            return Err(Error::Other(
                "Plugin does not have a GUI editor".to_string(),
            ));
        }

        // Close existing window if any. Keyed on the handle rather than `is_open()`, which also
        // reports `false` for a window the user already dismissed — that one still needs closing.
        if self.native_window.is_some() {
            self.close();
        }

        // Get plugin info for window title
        let plugin_info = self
            .plugin
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .info()
            .clone();

        // Try to get editor size
        let (width, height) = self
            .plugin
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .get_editor_size()
            .unwrap_or((800, 600));

        // Create native window
        #[cfg(target_os = "macos")]
        {
            // AppKit objects must be created on the main thread.
            let mtm = MainThreadMarker::new().ok_or_else(|| {
                Error::Other("plugin editor window must be opened on the main thread".to_string())
            })?;

            let frame = NSRect::new(
                NSPoint::new(100.0, 100.0),
                NSSize::new(width as f64, height as f64),
            );
            let style = NSWindowStyleMask::Titled
                | NSWindowStyleMask::Closable
                | NSWindowStyleMask::Miniaturizable;

            // SAFETY: standard AppKit window/view construction on the main thread.
            let window = unsafe {
                NSWindow::initWithContentRect_styleMask_backing_defer(
                    NSWindow::alloc(mtm),
                    frame,
                    style,
                    NSBackingStoreType::Buffered,
                    false,
                )
            };

            // Programmatic NSWindows default to `releasedWhenClosed = YES`; closing one would
            // then release it while our `Retained<NSWindow>` also releases on drop — a
            // double-free that crashes on close. We own the lifetime, so opt out.
            // SAFETY: standard AppKit setter on the main thread.
            unsafe { window.setReleasedWhenClosed(false) };

            let title = NSString::from_str(&format!("{} - VST3", plugin_info.name));
            window.setTitle(&title);

            // A container view, sized to the editor, that the plugin attaches its view into.
            let container_frame = NSRect::new(
                NSPoint::new(0.0, 0.0),
                NSSize::new(width as f64, height as f64),
            );
            let container_view = NSView::initWithFrame(NSView::alloc(mtm), container_frame);
            if let Some(content_view) = window.contentView() {
                content_view.addSubview(&container_view);
            }

            // Hand the plugin the container NSView to embed its editor in.
            // SAFETY: `container_view` is a live NSView this `PluginWindow` keeps alive (via
            // the retained window it was added to) for as long as the editor is attached —
            // `close()` detaches the editor before dropping the window.
            let window_handle = unsafe {
                crate::plugin::WindowHandle::from_nsview(
                    Retained::as_ptr(&container_view) as *mut std::ffi::c_void
                )
            };
            self.plugin
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .open_editor(window_handle)?;

            // Match the window to the editor size, then show and center it.
            window.setContentSize(container_frame.size);
            window.makeKeyAndOrderFront(None);
            window.center();

            self.native_window = Some(window);
            self.container_view = Some(container_view);
        }

        #[cfg(target_os = "windows")]
        {
            unsafe {
                use std::mem;
                use std::ptr;

                // Register window class if not already registered
                let class_name = "VST3PluginWindow\0".encode_utf16().collect::<Vec<u16>>();
                let mut wc: WNDCLASSEXW = mem::zeroed();
                wc.cbSize = mem::size_of::<WNDCLASSEXW>() as UINT;
                // The child editor covers everything below the 64-DIP host
                // toolbar. CS_HREDRAW/CS_VREDRAW would invalidate that entire
                // parent surface on every size transition; WM_SIZE already
                // resizes the child and explicitly invalidates only chrome.
                wc.style = CS_DROPSHADOW;
                wc.lpfnWndProc = Some(plugin_window_proc);
                wc.hInstance = GetModuleHandleW(ptr::null());
                wc.hCursor = LoadCursorW(ptr::null_mut(), IDC_ARROW);
                wc.lpszClassName = class_name.as_ptr();

                // Try to register, ignore if already registered
                RegisterClassExW(&wc);

                // Create window
                let window_title = format!("MiniStudio · {} · VST3\0", plugin_info.name);
                let window_name = window_title.encode_utf16().collect::<Vec<u16>>();

                let hwnd = CreateWindowExW(
                    WS_EX_APPWINDOW,
                    class_name.as_ptr(),
                    window_name.as_ptr(),
                    WS_POPUP | WS_MINIMIZEBOX | WS_CLIPCHILDREN,
                    CW_USEDEFAULT,
                    CW_USEDEFAULT,
                    width,
                    height + HOST_CHROME_HEIGHT_DIP,
                    ptr::null_mut(),
                    ptr::null_mut(),
                    GetModuleHandleW(ptr::null()),
                    ptr::null_mut(),
                );

                if hwnd.is_null() {
                    return Err(Error::Other("Failed to create native window".to_string()));
                }

                let dpi = GetDpiForWindow(hwnd);
                let chrome_height = scale_dip(HOST_CHROME_HEIGHT_DIP, dpi);
                SetWindowPos(
                    hwnd,
                    ptr::null_mut(),
                    0,
                    0,
                    width,
                    height + chrome_height,
                    SWP_NOMOVE | SWP_NOZORDER | SWP_NOACTIVATE,
                );
                let static_class = "STATIC\0".encode_utf16().collect::<Vec<u16>>();
                let container = CreateWindowExW(
                    0,
                    static_class.as_ptr(),
                    ptr::null(),
                    WS_CHILD | WS_VISIBLE | WS_CLIPCHILDREN | WS_CLIPSIBLINGS,
                    0,
                    chrome_height,
                    width,
                    height,
                    hwnd,
                    ptr::null_mut(),
                    GetModuleHandleW(ptr::null()),
                    ptr::null_mut(),
                );
                if container.is_null() {
                    DestroyWindow(hwnd);
                    return Err(Error::Other(
                        "Failed to create native editor container".to_string(),
                    ));
                }
                SetWindowLongPtrW(hwnd, GWLP_USERDATA, container as isize);

                // Try to open plugin editor.
                // SAFETY: `container` is a same-process, same-thread child of the host window.
                // It remains alive until after the editor is detached in `close()`.
                let window_handle =
                    crate::plugin::WindowHandle::from_hwnd(container as *mut std::ffi::c_void);
                let mut plugin = self.plugin.lock().unwrap_or_else(|p| p.into_inner());
                if let Some(scale_factor) = dpi_scale_factor(dpi) {
                    if let Err(error) = plugin.set_editor_scale_factor(scale_factor) {
                        drop(plugin);
                        DestroyWindow(hwnd);
                        return Err(error);
                    }
                }
                match plugin.open_editor(window_handle) {
                    Ok(()) => {
                        drop(plugin);
                        host_chrome_states()
                            .lock()
                            .unwrap_or_else(|poison| poison.into_inner())
                            .insert(
                                hwnd as usize,
                                HostChromeState {
                                    has_sidechain: plugin_info.audio_inputs > 1,
                                    ..HostChromeState::default()
                                },
                            );
                        ShowWindow(hwnd, SW_SHOW);
                        UpdateWindow(hwnd);
                        self.native_window = Some(hwnd);
                        self.editor_container = Some(container);
                    }
                    Err(e) => {
                        drop(plugin);
                        DestroyWindow(hwnd);
                        return Err(e);
                    }
                }
            }
        }

        #[cfg(target_os = "linux")]
        {
            use xcb::Xid;

            // Create an X11 window via XCB and embed the plugin editor into it using the
            // VST3 X11EmbedWindowID platform type (handled in plugin_impl::open_editor).
            let (connection, screen_number) = xcb::Connection::connect(None)
                .map_err(|e| Error::Other(format!("Failed to connect to X server: {e}")))?;
            let setup = connection.get_setup();
            let screen = setup
                .roots()
                .nth(screen_number as usize)
                .ok_or_else(|| Error::Other("No X11 screen found".to_string()))?;
            let window = connection.generate_id();

            connection
                .send_and_check_request(&xcb::x::CreateWindow {
                    depth: xcb::x::COPY_FROM_PARENT as u8,
                    wid: window,
                    parent: screen.root(),
                    x: 0,
                    y: 0,
                    width: width as u16,
                    height: height as u16,
                    border_width: 0,
                    class: xcb::x::WindowClass::InputOutput,
                    visual: screen.root_visual(),
                    value_list: &[
                        xcb::x::Cw::BackPixel(screen.white_pixel()),
                        xcb::x::Cw::EventMask(
                            xcb::x::EventMask::EXPOSURE | xcb::x::EventMask::KEY_PRESS,
                        ),
                    ],
                })
                .map_err(|e| Error::Other(format!("Failed to create X11 window: {e}")))?;

            // Window title.
            let title = format!("{} - VST3", plugin_info.name);
            connection.send_request(&xcb::x::ChangeProperty {
                mode: xcb::x::PropMode::Replace,
                window,
                property: xcb::x::ATOM_WM_NAME,
                r#type: xcb::x::ATOM_STRING,
                data: title.as_bytes(),
            });

            // Show the window, then attach the plugin editor to its X11 id.
            connection.send_request(&xcb::x::MapWindow { window });
            let _ = connection.flush();

            let handle = crate::plugin::WindowHandle::from_x11(window.resource_id());
            self.plugin
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .open_editor(handle)?;

            self.native_window = Some(XcbWindowState { connection, window });
        }

        #[cfg(target_os = "android")]
        {
            return Err(Error::Other(
                "PluginWindow::open() is not supported on Android".to_string(),
            ));
        }

        Ok(())
    }

    /// Pump the editor's window-level traffic that has to cross into the plugin.
    ///
    /// **Call this from your UI event loop, once per frame, while the window is open.** Two
    /// things depend on it:
    ///
    /// - **Plugin-initiated resizes.** A resizable editor (VSTGUI zoom, a "big view" toggle)
    ///   asks the host to resize through `IPlugFrame::resizeView`. This applies the request to
    ///   the native window, so the editor is not clipped by a window that never grew.
    /// - **Windows DPI changes.** `WM_DPICHANGED` applies its suggested native rectangle
    ///   immediately in the window procedure and queues the new content scale for here; making
    ///   the COM call outside the window procedure prevents reentrant deadlocks on the plugin
    ///   mutex.
    ///
    /// Deliberately non-blocking: if the plugin mutex is held elsewhere (the audio callback
    /// holds it per block) this returns without doing anything, and the pending work is picked
    /// up on the next call.
    ///
    /// [`Self::is_open`] and [`Self::closed_by_user`] do *not* run this — they stay lock-free
    /// so they are safe to call while holding the plugin lock.
    pub fn service_platform_events(&self) -> Result<()> {
        if self.native_window.is_none() {
            return Ok(());
        }
        let Some(mut plugin) = self.try_lock_plugin() else {
            return Ok(());
        };

        let scale_result = self
            .take_pending_scale_factor()
            .map(|factor| plugin.set_editor_scale_factor(factor));
        let resize = plugin.take_editor_resize_request();

        // Released before touching the native window: on Windows, resizing it may synchronously
        // dispatch window messages back into host code that wants this same lock.
        drop(plugin);

        if let Some((width, height)) = resize {
            self.resize_native_window(width, height);
        }
        match scale_result {
            Some(Err(error)) => Err(error),
            _ => Ok(()),
        }
    }

    /// Drain user intent from the host-owned native toolbar without calling into vendor code.
    /// This is consumed over the isolation control channel and is never used by the audio path.
    pub fn take_host_actions(&self) -> Vec<EditorHostAction> {
        #[cfg(target_os = "windows")]
        {
            let Some(hwnd) = self.native_window else {
                return Vec::new();
            };
            return host_chrome_states()
                .lock()
                .ok()
                .and_then(|mut states| {
                    states
                        .get_mut(&(hwnd as usize))
                        .map(|state| std::mem::take(&mut state.actions))
                })
                .unwrap_or_default();
        }
        #[cfg(not(target_os = "windows"))]
        Vec::new()
    }

    /// Apply authoritative DAW state to the lightweight native indicators.
    pub fn set_host_state(&self, state: EditorHostState) {
        #[cfg(target_os = "windows")]
        if let Some(hwnd) = self.native_window {
            let changed = host_chrome_states().lock().ok().and_then(|mut states| {
                let chrome = states.get_mut(&(hwnd as usize))?;
                let next_automation = state.automation.min(3);
                let changed = (
                    chrome.bypassed != state.bypassed,
                    chrome.automation,
                    next_automation,
                );
                chrome.bypassed = state.bypassed;
                chrome.automation = next_automation;
                Some(changed)
            });
            if let Some((bypass_changed, old_automation, new_automation)) = changed {
                unsafe {
                    if bypass_changed {
                        invalidate_host_button(hwnd, HostChromeButton::Power);
                        invalidate_host_button(hwnd, HostChromeButton::Bypass);
                    }
                    if old_automation != new_automation {
                        let automation_button = |mode| match mode {
                            1 => HostChromeButton::AutomationWrite,
                            2 => HostChromeButton::AutomationRead,
                            3 => HostChromeButton::AutomationLatch,
                            _ => HostChromeButton::AutomationOff,
                        };
                        invalidate_host_button(hwnd, automation_button(old_automation));
                        invalidate_host_button(hwnd, automation_button(new_automation));
                    }
                }
            }
        }
        #[cfg(not(target_os = "windows"))]
        let _ = state;
    }

    /// Take the plugin lock without blocking, recovering a lock poisoned by an unrelated panic
    /// (a poisoned mutex is permanent, and treating it as failure would stop servicing the
    /// editor for the rest of the session).
    fn try_lock_plugin(&self) -> Option<std::sync::MutexGuard<'_, Plugin>> {
        match self.plugin.try_lock() {
            Ok(guard) => Some(guard),
            Err(std::sync::TryLockError::Poisoned(poison)) => Some(poison.into_inner()),
            Err(std::sync::TryLockError::WouldBlock) => None,
        }
    }

    /// The newest DPI the window procedure recorded for this window, as a VST3 content scale.
    #[cfg(target_os = "windows")]
    fn take_pending_scale_factor(&self) -> Option<f32> {
        let hwnd = self.native_window?;
        dpi_changes()
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .remove(&(hwnd as usize))
            .and_then(dpi_scale_factor)
    }

    /// Only Windows reports DPI changes through the window procedure.
    #[cfg(not(target_os = "windows"))]
    fn take_pending_scale_factor(&self) -> Option<f32> {
        None
    }

    /// Resize the native window so it fits an editor of `width` x `height` pixels.
    ///
    /// The plugin has already resized its own view (the host answered `resizeView` with
    /// `onSize`); this catches the window up.
    fn resize_native_window(&self, width: i32, height: i32) {
        if width <= 0 || height <= 0 {
            return;
        }
        log::debug!("editor asked the host to resize its window to {width}x{height}");

        #[cfg(target_os = "macos")]
        {
            let Some(window) = self.native_window.as_ref() else {
                return;
            };
            let size = NSSize::new(width as f64, height as f64);
            if let Some(container) = self.container_view.as_ref() {
                container.setFrame(NSRect::new(NSPoint::new(0.0, 0.0), size));
            }
            window.setContentSize(size);
        }

        #[cfg(target_os = "windows")]
        {
            let Some(hwnd) = self.native_window else {
                return;
            };
            unsafe {
                SetWindowPos(
                    hwnd,
                    std::ptr::null_mut(),
                    0,
                    0,
                    width,
                    height + host_chrome_height(hwnd),
                    SWP_NOMOVE | SWP_NOZORDER | SWP_NOACTIVATE,
                );
            }
        }

        #[cfg(target_os = "linux")]
        {
            let Some(state) = self.native_window.as_ref() else {
                return;
            };
            state.connection.send_request(&xcb::x::ConfigureWindow {
                window: state.window,
                value_list: &[
                    xcb::x::ConfigWindow::Width(width as u32),
                    xcb::x::ConfigWindow::Height(height as u32),
                ],
            });
            let _ = state.connection.flush();
        }

        #[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
        let _ = (width, height);
    }

    /// Close the plugin window
    pub fn close(&mut self) {
        // Detach the plugin editor first — it must never outlive the window it is embedded in.
        // A poisoned lock (an earlier panic on the audio thread) is recovered rather than
        // skipped, exactly as every other lock site here does: skipping it would destroy the
        // native window with the plugin's view still attached to it.
        let _ = self
            .plugin
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .close_editor();

        // Then close the native window
        #[cfg(target_os = "macos")]
        {
            self.container_view = None;
            if let Some(window) = self.native_window.take() {
                window.close();
            }
        }

        #[cfg(target_os = "windows")]
        {
            self.editor_container = None;
            if let Some(hwnd) = self.native_window.take() {
                if let Ok(mut requests) = close_requests().lock() {
                    requests.remove(&(hwnd as usize));
                }
                if let Ok(mut changes) = dpi_changes().lock() {
                    changes.remove(&(hwnd as usize));
                }
                if let Ok(mut states) = host_chrome_states().lock() {
                    states.remove(&(hwnd as usize));
                }
                unsafe {
                    DestroyWindow(hwnd);
                }
            }
        }

        #[cfg(target_os = "linux")]
        {
            if let Some(state) = self.native_window.take() {
                state.connection.send_request(&xcb::x::UnmapWindow {
                    window: state.window,
                });
                state.connection.send_request(&xcb::x::DestroyWindow {
                    window: state.window,
                });
                let _ = state.connection.flush();
            }
        }

        #[cfg(target_os = "android")]
        {
            let _ = self.native_window.take();
        }
    }

    /// Check if the window is currently open.
    ///
    /// Reports `false` once the user has dismissed the window themselves, even though the
    /// handle is still around — see [`Self::closed_by_user`].
    ///
    /// Reads native window state only: it never takes the plugin lock, so it is safe to call
    /// while holding it. Pair it with [`Self::service_platform_events`], which does the work
    /// that has to reach the plugin.
    pub fn is_open(&self) -> bool {
        self.native_window.is_some() && !self.native_window_dismissed()
    }

    /// Whether the user closed the window themselves, through its title-bar close button.
    ///
    /// Neither the plugin nor the host is told when that happens, so a host that tracks "the
    /// editor is open" in its own UI should poll this each frame and drop or
    /// [`close`](Self::close) the window when it reports `true`. Otherwise the editor stays
    /// attached to a window that is gone from the screen.
    ///
    /// Cheap and lock-free in the same sense as [`Self::is_open`]: native window state only,
    /// never the plugin lock.
    pub fn closed_by_user(&self) -> bool {
        self.native_window_dismissed()
    }

    /// Platform probe behind [`Self::closed_by_user`].
    #[cfg(target_os = "macos")]
    fn native_window_dismissed(&self) -> bool {
        let Some(window) = self.native_window.as_ref() else {
            return false;
        };
        // A closed window is ordered out — but so is a miniaturized one, and so is every window
        // of a hidden application. Neither of those means the user dismissed the editor.
        if window.isVisible() || window.isMiniaturized() {
            return false;
        }
        match MainThreadMarker::new() {
            Some(mtm) => !NSApplication::sharedApplication(mtm).isHidden(),
            // AppKit state can only be read from the main thread; assume the window still stands.
            None => false,
        }
    }

    /// Platform probe behind [`Self::closed_by_user`].
    #[cfg(target_os = "windows")]
    fn native_window_dismissed(&self) -> bool {
        let Some(hwnd) = self.native_window else {
            return false;
        };
        close_requests()
            .lock()
            .is_ok_and(|requests| requests.contains(&(hwnd as usize)))
    }

    /// Platform probe behind [`Self::closed_by_user`]. X11 and Android editor windows carry no
    /// close affordance of their own, so there is nothing to observe.
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    fn native_window_dismissed(&self) -> bool {
        false
    }
}

impl Drop for PluginWindow {
    fn drop(&mut self) {
        self.close();
    }
}

/// Builder for creating plugin windows with egui integration
#[cfg(feature = "egui-widgets")]
pub struct PluginWindowBuilder {
    plugin: Arc<Mutex<Plugin>>,
}

#[cfg(feature = "egui-widgets")]
impl PluginWindowBuilder {
    /// Create a new builder for the given plugin
    pub fn new(plugin: Arc<Mutex<Plugin>>) -> Self {
        Self { plugin }
    }

    /// Build and open a standalone plugin window
    pub fn open_standalone(&self) -> Result<PluginWindow> {
        let mut window = PluginWindow::new(self.plugin.clone());
        window.open()?;
        Ok(window)
    }
}

#[cfg(test)]
mod tests {
    use super::dpi_scale_factor;

    #[test]
    fn windows_dpi_converts_to_vst_content_scale() {
        assert_eq!(dpi_scale_factor(0), None);
        assert_eq!(dpi_scale_factor(96), Some(1.0));
        assert_eq!(dpi_scale_factor(120), Some(1.25));
        assert_eq!(dpi_scale_factor(144), Some(1.5));
        assert_eq!(dpi_scale_factor(192), Some(2.0));
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn wm_dpi_wparam_uses_horizontal_dpi() {
        let packed = (192usize << 16) | 144;
        assert_eq!(super::dpi_from_wparam(packed), Some(144));
        assert_eq!(super::dpi_from_wparam(0), None);
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn host_chrome_keeps_automation_controls_right_aligned() {
        use super::{HostChromeButton, HostChromeLayout};

        let layout = HostChromeLayout::new(900, 64, 96, true);
        let off = layout
            .button_rect(HostChromeButton::AutomationOff)
            .unwrap();
        let latch = layout
            .button_rect(HostChromeButton::AutomationLatch)
            .unwrap();
        let label = layout.automation_label_rect().unwrap();

        assert_eq!(off.left, 900 - 24 * 4);
        assert_eq!(latch.right, 900);
        assert_eq!(label.right, off.left);
        assert!(
            layout
                .button_rect(HostChromeButton::Sidechain)
                .unwrap()
                .right
                < label.left
        );
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn narrow_host_chrome_preserves_automation_hit_targets() {
        use super::{HostChromeButton, HostChromeLayout};

        let layout = HostChromeLayout::new(300, 64, 96, true);
        assert!(layout
            .button_rect(HostChromeButton::Sidechain)
            .is_none());
        assert_eq!(
            layout
                .button_rect(HostChromeButton::AutomationLatch)
                .unwrap()
                .right,
            300
        );
    }
}
