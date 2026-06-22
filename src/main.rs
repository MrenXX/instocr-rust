#![windows_subsystem = "windows"]

use std::error::Error;
use std::ffi::c_void;
use std::fs;
use std::mem::size_of;
use std::path::PathBuf;
use std::ptr::{null, null_mut};
use std::sync::mpsc::{self, Sender};
use std::thread;
use std::time::Instant;

use windows::core::{Error as WinError, Result as WinResult, PCWSTR};
use windows::Globalization::Language;
use windows::Graphics::Imaging::{BitmapPixelFormat, SoftwareBitmap};
use windows::Media::Ocr::{OcrEngine, OcrResult};
use windows::Storage::Streams::DataWriter;
use windows::Win32::Foundation::{
    CloseHandle, GetLastError, BOOL, COLORREF, ERROR_ALREADY_EXISTS, ERROR_SUCCESS, HANDLE,
    HINSTANCE, HWND, LPARAM, LRESULT, POINT, WPARAM,
};
use windows::Win32::Graphics::Gdi::{
    BeginPaint, BitBlt, CreateCompatibleBitmap, CreateCompatibleDC, CreatePen, CreateSolidBrush,
    DeleteDC, DeleteObject, EndPaint, GetDC, GetDIBits, GetStockObject, InvalidateRect, Rectangle,
    ReleaseDC, SelectObject, SetBkMode, SetTextColor, StretchDIBits, TextOutW, BITMAPINFO,
    BITMAPINFOHEADER, BI_RGB, DIB_RGB_COLORS, HBRUSH, HDC, HOLLOW_BRUSH, PAINTSTRUCT, PS_SOLID,
    SRCCOPY, TRANSPARENT,
};
use windows::Win32::System::Com::{CoInitializeEx, CoUninitialize, COINIT_MULTITHREADED};
use windows::Win32::System::DataExchange::{
    CloseClipboard, EmptyClipboard, OpenClipboard, SetClipboardData,
};
use windows::Win32::System::Memory::{GlobalAlloc, GlobalLock, GlobalUnlock, GMEM_MOVEABLE};
use windows::Win32::System::Ole::CF_UNICODETEXT;
use windows::Win32::System::ProcessStatus::{GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS_EX};
use windows::Win32::System::Registry::{
    RegCloseKey, RegCreateKeyExW, RegDeleteValueW, RegOpenKeyExW, RegQueryValueExW, RegSetValueExW,
    HKEY, HKEY_CURRENT_USER, KEY_READ, KEY_SET_VALUE, REG_OPTION_NON_VOLATILE, REG_SAM_FLAGS,
    REG_SZ,
};
use windows::Win32::System::Threading::{CreateMutexW, GetCurrentProcess};
use windows::Win32::UI::HiDpi::{
    SetProcessDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    RegisterHotKey, ReleaseCapture, SetCapture, UnregisterHotKey, HOT_KEY_MODIFIERS, MOD_ALT,
    MOD_CONTROL, MOD_NOREPEAT, MOD_SHIFT, VK_ESCAPE,
};
use windows::Win32::UI::Shell::{
    Shell_NotifyIconW, NIF_ICON, NIF_MESSAGE, NIF_TIP, NIM_ADD, NIM_DELETE, NIM_MODIFY,
    NOTIFYICONDATAW,
};
use windows::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, CreatePopupMenu, CreateWindowExW, DefWindowProcW, DestroyMenu, DestroyWindow,
    DispatchMessageW, GetCursorPos, GetMessageW, GetSystemMetrics, GetWindowLongPtrW, KillTimer,
    LoadCursorW, LoadIconW, MessageBoxW, PostMessageW, PostQuitMessage, RegisterClassW,
    SetForegroundWindow, SetTimer, SetWindowLongPtrW, ShowWindow, TrackPopupMenu, TranslateMessage,
    CREATESTRUCTW, GWLP_USERDATA, HMENU, IDC_ARROW, IDC_CROSS, IDI_APPLICATION, MB_ICONERROR,
    MB_OK, MF_CHECKED, MF_SEPARATOR, MF_STRING, MF_UNCHECKED, MSG, SM_CXVIRTUALSCREEN,
    SM_CYVIRTUALSCREEN, SM_XVIRTUALSCREEN, SM_YVIRTUALSCREEN, SW_SHOW, SW_SHOWNOACTIVATE,
    TPM_LEFTALIGN, TPM_RIGHTBUTTON, WINDOW_EX_STYLE, WINDOW_STYLE, WM_APP, WM_COMMAND, WM_DESTROY,
    WM_HOTKEY, WM_KEYDOWN, WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MOUSEMOVE, WM_NCCREATE, WM_NCDESTROY,
    WM_PAINT, WM_RBUTTONDOWN, WM_RBUTTONUP, WM_TIMER, WNDCLASSW, WS_EX_NOACTIVATE,
    WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_POPUP,
};

type AnyResult<T> = std::result::Result<T, Box<dyn Error>>;

const HOTKEY_CAPTURE: i32 = 1;
const HOTKEY_CYCLE_LANGUAGE: i32 = 2;
const TRAY_ID: u32 = 1;
const WM_TRAY: u32 = WM_APP + 1;
const WM_SCREEN_CAPTURE_DONE: u32 = WM_APP + 2;
const WM_SELECTION_DONE: u32 = WM_APP + 3;
const WM_OCR_DONE: u32 = WM_APP + 4;
const WM_WORKER_STATUS: u32 = WM_APP + 5;
const TIMER_IDLE_METRICS: usize = 1;
const TIMER_STATUS_HIDE: usize = 2;

const IDM_CAPTURE: usize = 100;
const IDM_CYCLE_LANGUAGE: usize = 101;
const IDM_TOGGLE_STARTUP: usize = 102;
const IDM_LIST_LANGUAGES: usize = 103;
const IDM_EXIT: usize = 199;
const IDM_LANGUAGE_BASE: usize = 300;

const STARTUP_VALUE_NAME: &str = "InstOCR";
const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
const DEFAULT_CAPTURE_HOTKEY: &str = "Ctrl+Alt+D";
const DEFAULT_CYCLE_HOTKEY: &str = "Ctrl+Alt+W";

fn main() -> AnyResult<()> {
    unsafe {
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        CoInitializeEx(None, COINIT_MULTITHREADED).ok()?;
    }

    let result = run();
    if let Err(error) = &result {
        show_fatal_error(&format!("{error}"));
    }

    unsafe {
        CoUninitialize();
    }

    result
}

fn show_fatal_error(message: &str) {
    unsafe {
        let title = wide_null("InstOCR Rust startup failed");
        let message = wide_null(message);
        let _ = MessageBoxW(
            null_hwnd(),
            PCWSTR(message.as_ptr()),
            PCWSTR(title.as_ptr()),
            MB_OK | MB_ICONERROR,
        );
    }
}

fn run() -> AnyResult<()> {
    let args = std::env::args().skip(1).collect::<Vec<_>>();
    let resident_warm = args.iter().any(|arg| arg == "--resident-warm");

    if args.iter().any(|arg| arg != "--resident-warm") {
        return Err(
            "Use instocr-cli.exe for --measure, --list-languages, and --capture-rect.".into(),
        );
    }

    run_resident_app(resident_warm)
}

fn run_resident_app(resident_warm: bool) -> AnyResult<()> {
    let _single_instance = SingleInstanceGuard::acquire()?;
    println!("InstOCR Rust native prototype starting.");
    println!(
        "mode: {}",
        if resident_warm {
            "resident-warm"
        } else {
            "on-demand"
        }
    );
    print_memory("startup");

    unsafe {
        register_window_class("InstOcrRustHiddenWindow", Some(hidden_wnd_proc), IDC_ARROW)?;
        register_window_class(
            "InstOcrRustSelectionOverlay",
            Some(overlay_wnd_proc),
            IDC_CROSS,
        )?;
        register_window_class("InstOcrRustStatusPopup", Some(status_wnd_proc), IDC_ARROW)?;
    }

    let mut app = Box::new(AppState::new(resident_warm)?);
    print_languages(&app.ocr);
    print_memory("after-language-enumeration");

    let hwnd = unsafe { create_hidden_window(app.as_mut() as *mut AppState)? };
    app.hwnd = hwnd;
    app.install_tray_icon();
    app.register_hotkeys();
    if resident_warm {
        app.warm_ocr();
    } else {
        app.update_status("Ready (OCR on-demand)");
        println!(
            "warmup: skipped (on-demand; pass --resident-warm to warm OCR engines at startup)"
        );
        print_memory("after-startup-warm-skipped");
    }

    unsafe {
        SetTimer(hwnd, TIMER_IDLE_METRICS, 10_000, None);
    }

    println!("Ready. Ctrl+Alt+D captures; Ctrl+Alt+W cycles OCR language.");

    unsafe {
        let mut msg = MSG::default();
        while GetMessageW(&mut msg, null_hwnd(), 0, 0).as_bool() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }

    app.cleanup();
    Ok(())
}

struct SingleInstanceGuard {
    handle: HANDLE,
}

impl SingleInstanceGuard {
    fn acquire() -> AnyResult<Self> {
        unsafe {
            let name = wide_null("Local\\InstOCR_Rust_Native");
            let handle = CreateMutexW(None, true, PCWSTR(name.as_ptr()))?;
            if GetLastError() == ERROR_ALREADY_EXISTS {
                return Err("InstOCR Rust is already running.".into());
            }
            Ok(Self { handle })
        }
    }
}

impl Drop for SingleInstanceGuard {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseHandle(self.handle);
        }
    }
}

struct AppState {
    hwnd: HWND,
    ocr: OcrController,
    worker: OcrWorker,
    settings: RustSettings,
    capture_hotkey: Hotkey,
    cycle_hotkey: Hotkey,
    resident_warm: bool,
    tray_added: bool,
    status_popup: Option<HWND>,
}

#[derive(Clone)]
struct RustSettings {
    language_tag: Option<String>,
    capture_hotkey: String,
    cycle_hotkey: String,
}

impl Default for RustSettings {
    fn default() -> Self {
        Self {
            language_tag: None,
            capture_hotkey: DEFAULT_CAPTURE_HOTKEY.to_string(),
            cycle_hotkey: DEFAULT_CYCLE_HOTKEY.to_string(),
        }
    }
}

impl RustSettings {
    fn load() -> Self {
        let path = rust_settings_path();
        let Ok(text) = fs::read_to_string(path) else {
            return Self::default();
        };

        let mut settings = Self::default();
        for line in text.lines() {
            if let Some(value) = line.strip_prefix("language_tag=") {
                settings.language_tag =
                    Some(value.trim().to_string()).filter(|value| !value.is_empty());
            } else if let Some(value) = line.strip_prefix("capture_hotkey=") {
                settings.capture_hotkey = value.trim().to_string();
            } else if let Some(value) = line.strip_prefix("cycle_hotkey=") {
                settings.cycle_hotkey = value.trim().to_string();
            }
        }

        if Hotkey::parse(&settings.capture_hotkey).is_err() {
            settings.capture_hotkey = DEFAULT_CAPTURE_HOTKEY.to_string();
        }
        if Hotkey::parse(&settings.cycle_hotkey).is_err() {
            settings.cycle_hotkey = DEFAULT_CYCLE_HOTKEY.to_string();
        }
        if settings
            .capture_hotkey
            .eq_ignore_ascii_case(&settings.cycle_hotkey)
        {
            settings.capture_hotkey = DEFAULT_CAPTURE_HOTKEY.to_string();
            settings.cycle_hotkey = DEFAULT_CYCLE_HOTKEY.to_string();
        }
        if settings
            .capture_hotkey
            .eq_ignore_ascii_case(&settings.cycle_hotkey)
        {
            settings.capture_hotkey = DEFAULT_CAPTURE_HOTKEY.to_string();
            settings.cycle_hotkey = DEFAULT_CYCLE_HOTKEY.to_string();
        }

        settings
    }

    fn save(&self) {
        let path = rust_settings_path();
        if let Some(parent) = path.parent() {
            let _ = fs::create_dir_all(parent);
        }

        let value = self.language_tag.as_deref().unwrap_or("");
        let text = format!(
            "language_tag={value}\ncapture_hotkey={}\ncycle_hotkey={}\n",
            self.capture_hotkey, self.cycle_hotkey
        );
        let _ = fs::write(path, text);
    }
}

fn rust_settings_path() -> PathBuf {
    let base = std::env::var_os("APPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));
    base.join("InstOCR").join("rust-settings.txt")
}

#[derive(Clone)]
struct Hotkey {
    modifiers: u32,
    virtual_key: u32,
    display: String,
}

impl Hotkey {
    fn parse(value: &str) -> Result<Self, String> {
        let mut modifiers = 0u32;
        let mut key: Option<(u32, String)> = None;

        for raw in value
            .split('+')
            .map(str::trim)
            .filter(|part| !part.is_empty())
        {
            match raw.to_ascii_uppercase().as_str() {
                "CTRL" | "CONTROL" => modifiers |= MOD_CONTROL.0,
                "ALT" => modifiers |= MOD_ALT.0,
                "SHIFT" => modifiers |= MOD_SHIFT.0,
                "WIN" | "WINDOWS" | "META" => {
                    return Err("Win hotkeys are reserved by Windows".to_string())
                }
                token => {
                    if key.is_some() {
                        return Err("Hotkey can only contain one non-modifier key".to_string());
                    }
                    key = Some(parse_virtual_key(token)?);
                }
            }
        }

        if modifiers & (MOD_CONTROL.0 | MOD_ALT.0) == 0 {
            return Err("Hotkey must include Ctrl or Alt".to_string());
        }

        let Some((virtual_key, key_display)) = key else {
            return Err("Hotkey must include a key".to_string());
        };

        if virtual_key == 0x7B {
            return Err("F12 is reserved for debuggers".to_string());
        }

        let mut parts = Vec::new();
        if modifiers & MOD_CONTROL.0 != 0 {
            parts.push("Ctrl".to_string());
        }
        if modifiers & MOD_ALT.0 != 0 {
            parts.push("Alt".to_string());
        }
        if modifiers & MOD_SHIFT.0 != 0 {
            parts.push("Shift".to_string());
        }
        parts.push(key_display);

        Ok(Self {
            modifiers,
            virtual_key,
            display: parts.join("+"),
        })
    }
}

fn parse_virtual_key(token: &str) -> Result<(u32, String), String> {
    if token.len() == 1 {
        let ch = token.chars().next().expect("token has one char");
        if ch.is_ascii_alphanumeric() {
            let upper = ch.to_ascii_uppercase();
            return Ok((upper as u32, upper.to_string()));
        }
    }

    if let Some(number) = token
        .strip_prefix('F')
        .and_then(|value| value.parse::<u32>().ok())
    {
        if (1..=24).contains(&number) {
            return Ok((0x70 + number - 1, format!("F{number}")));
        }
    }

    match token {
        "SPACE" => Ok((0x20, "Space".to_string())),
        "TAB" => Ok((0x09, "Tab".to_string())),
        "ENTER" => Ok((0x0D, "Enter".to_string())),
        "ESC" | "ESCAPE" => Ok((0x1B, "Esc".to_string())),
        "PRINTSCREEN" | "PRTSC" => Ok((0x2C, "PrintScreen".to_string())),
        _ => Err(format!("Unsupported key: {token}")),
    }
}

impl AppState {
    fn new(resident_warm: bool) -> WinResult<Self> {
        let settings = RustSettings::load();
        let mut ocr = OcrController::new()?;
        ocr.set_current_language_tag(settings.language_tag.as_deref());
        let capture_hotkey = Hotkey::parse(&settings.capture_hotkey).unwrap_or_else(|_| {
            Hotkey::parse(DEFAULT_CAPTURE_HOTKEY).expect("default hotkey valid")
        });
        let cycle_hotkey = Hotkey::parse(&settings.cycle_hotkey)
            .unwrap_or_else(|_| Hotkey::parse(DEFAULT_CYCLE_HOTKEY).expect("default hotkey valid"));
        Ok(Self {
            hwnd: null_hwnd(),
            ocr,
            worker: OcrWorker::start(),
            settings,
            capture_hotkey,
            cycle_hotkey,
            resident_warm,
            tray_added: false,
            status_popup: None,
        })
    }

    fn register_hotkeys(&mut self) {
        self.capture_hotkey = self.register_hotkey_or_default(
            HOTKEY_CAPTURE,
            self.capture_hotkey.clone(),
            DEFAULT_CAPTURE_HOTKEY,
            "capture",
        );
        self.cycle_hotkey = self.register_hotkey_or_default(
            HOTKEY_CYCLE_LANGUAGE,
            self.cycle_hotkey.clone(),
            DEFAULT_CYCLE_HOTKEY,
            "language cycle",
        );
        self.settings.capture_hotkey = self.capture_hotkey.display.clone();
        self.settings.cycle_hotkey = self.cycle_hotkey.display.clone();
        self.settings.save();
    }

    fn register_hotkey_or_default(
        &mut self,
        id: i32,
        configured: Hotkey,
        default_text: &str,
        label: &str,
    ) -> Hotkey {
        match self.try_register_hotkey(id, &configured) {
            Ok(()) => {
                println!("hotkey registered: {label} {}", configured.display);
                configured
            }
            Err(error) => {
                let default_hotkey = Hotkey::parse(default_text).expect("default hotkey valid");
                let message = format!(
                    "{} hotkey {} unavailable: {}. Falling back to {}.",
                    label, configured.display, error, default_hotkey.display
                );
                self.show_status_popup(&message);
                if self.try_register_hotkey(id, &default_hotkey).is_err() {
                    self.show_status_popup(&format!(
                        "{} hotkey unavailable. Run Configure Hotkeys.",
                        label
                    ));
                }
                default_hotkey
            }
        }
    }

    fn try_register_hotkey(&self, id: i32, hotkey: &Hotkey) -> WinResult<()> {
        unsafe {
            RegisterHotKey(
                self.hwnd,
                id,
                HOT_KEY_MODIFIERS(hotkey.modifiers | MOD_NOREPEAT.0),
                hotkey.virtual_key,
            )
            .map(|_| ())
        }
    }

    fn install_tray_icon(&mut self) {
        unsafe {
            let mut data = notify_icon_data(self.hwnd, "InstOCR Rust: ready");
            data.uFlags = NIF_MESSAGE | NIF_ICON | NIF_TIP;
            data.uCallbackMessage = WM_TRAY;
            data.hIcon = LoadIconW(null_hinstance(), IDI_APPLICATION).unwrap_or_default();
            if Shell_NotifyIconW(NIM_ADD, &data).as_bool() {
                self.tray_added = true;
                println!("tray icon installed");
            } else {
                eprintln!("Shell_NotifyIconW(NIM_ADD) failed; continuing with console status only");
            }
        }
    }

    fn update_status(&self, status: &str) {
        println!("status: {status}");
        if self.tray_added {
            unsafe {
                let mut data = notify_icon_data(self.hwnd, status);
                data.uFlags = NIF_TIP;
                let _ = Shell_NotifyIconW(NIM_MODIFY, &data);
            }
        }
    }

    fn show_status_popup(&mut self, status: &str) {
        self.update_status(status);
        unsafe {
            if let Some(hwnd) = self.status_popup.take() {
                DestroyWindow(hwnd).ok();
            }

            match show_status_popup(self.hwnd, status) {
                Ok(hwnd) => self.status_popup = Some(hwnd),
                Err(error) => eprintln!("status popup failed: {error}"),
            }
        }
    }

    fn hide_status_popup(&mut self) {
        unsafe {
            if let Some(hwnd) = self.status_popup.take() {
                DestroyWindow(hwnd).ok();
            }
        }
    }

    fn warm_ocr(&mut self) {
        self.update_status("Warming OCR...");
        if let Err(error) = self.worker.warm_current(self.hwnd) {
            eprintln!("failed to queue OCR warmup: {error}");
            self.update_status("OCR warmup queue failed");
        }
    }

    fn start_capture(&mut self) {
        self.hide_status_popup();
        self.update_status("Capturing screen...");
        spawn_screen_capture(self.hwnd);
    }

    fn handle_screen_capture_result(&mut self, message: CaptureWorkerResult) {
        match message.result {
            Ok(captured) => {
                let overlay_start = Instant::now();
                match unsafe { show_overlay(self.hwnd, captured.capture, captured.capture_ms) } {
                    Ok(_) => {
                        self.update_status("Drag a rectangle; Esc/right-click cancels");
                        println!(
                            "capture-freeze: gdi={:.2}ms overlay={:.2}ms",
                            captured.capture_ms,
                            elapsed_ms(overlay_start)
                        );
                    }
                    Err(error) => {
                        eprintln!("overlay failed: {error}");
                        self.update_status("Capture overlay failed");
                    }
                }
            }
            Err(error) => {
                eprintln!("capture failed: {error}");
                self.update_status("Capture failed");
            }
        }
    }

    fn cycle_language(&mut self) {
        let start = Instant::now();
        let label = self.ocr.cycle_language();
        self.show_status_popup(&format!("OCR language: {label}"));
        println!(
            "language-cycle: active=\"{}\" elapsed={:.2}ms",
            label,
            elapsed_ms(start)
        );
        if let Err(error) =
            self.worker
                .set_language(self.hwnd, self.ocr.current_index(), self.resident_warm)
        {
            eprintln!("failed to queue OCR language change: {error}");
        }
        RustSettings {
            language_tag: self.ocr.current_language_tag(),
            capture_hotkey: self.settings.capture_hotkey.clone(),
            cycle_hotkey: self.settings.cycle_hotkey.clone(),
        }
        .save();
        print_memory("after-language-cycle");
    }

    fn set_language_index(&mut self, index: usize) {
        self.ocr.set_current_index(index);
        let label = self.ocr.current_label();
        self.show_status_popup(&format!("OCR language: {label}"));
        if let Err(error) =
            self.worker
                .set_language(self.hwnd, self.ocr.current_index(), self.resident_warm)
        {
            eprintln!("failed to queue OCR language change: {error}");
        }
        RustSettings {
            language_tag: self.ocr.current_language_tag(),
            capture_hotkey: self.settings.capture_hotkey.clone(),
            cycle_hotkey: self.settings.cycle_hotkey.clone(),
        }
        .save();
    }

    fn handle_selected_image(&mut self, selected: SelectedImage) {
        self.update_status("Running OCR...");
        if let Err(error) = self.worker.recognize(self.hwnd, selected) {
            eprintln!("failed to queue OCR work: {error}");
            self.update_status("OCR queue failed");
        }
    }

    fn handle_ocr_result(&mut self, message: OcrWorkerResult) {
        match message.result {
            Ok(success) => {
                if let Some(error) = &success.clipboard_error {
                    eprintln!("clipboard copy failed: {error}");
                }
                println!(
                    "capture-ocr: rect=({},{} {}x{}) capture={:.2}ms convert={:.2}ms engine={:.2}ms ocr={:.2}ms clipboard={:.2}ms total_after_selection={:.2}ms text=\"{}\"",
                    message.screen_rect.left,
                    message.screen_rect.top,
                    message.width,
                    message.height,
                    message.capture_ms,
                    success.outcome.convert_ms,
                    success.outcome.engine_ms,
                    success.outcome.ocr_ms,
                    success.clipboard_ms,
                    success.outcome.total_ms + success.clipboard_ms,
                    success.outcome.text.replace('\n', " ")
                );
                if success.clipboard_error.is_some() {
                    self.show_status_popup("OCR done; clipboard failed");
                } else {
                    self.show_status_popup("OCR copied to clipboard");
                }
                print_memory("after-capture-ocr");
            }
            Err(error) => {
                eprintln!("OCR failed: {error}");
                self.show_status_popup(&format!("OCR failed: {error}"));
            }
        }
    }

    fn show_menu(&mut self) {
        unsafe {
            let menu = match CreatePopupMenu() {
                Ok(menu) => menu,
                Err(error) => {
                    eprintln!("CreatePopupMenu failed: {error}");
                    return;
                }
            };

            append_menu(
                menu,
                IDM_CAPTURE,
                &format!("Capture\t{}", self.capture_hotkey.display),
            );
            append_menu(
                menu,
                IDM_CYCLE_LANGUAGE,
                &format!("Cycle OCR language\t{}", self.cycle_hotkey.display),
            );
            append_menu(menu, IDM_LIST_LANGUAGES, "List OCR languages");
            AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR(null())).ok();
            append_language_menu(menu, &self.ocr);
            AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR(null())).ok();
            let startup_label = if is_startup_enabled() {
                "Disable run at startup"
            } else {
                "Enable run at startup"
            };
            append_menu(menu, IDM_TOGGLE_STARTUP, startup_label);
            AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR(null())).ok();
            append_menu(menu, IDM_EXIT, "Exit");

            let mut point = POINT::default();
            if let Err(error) = GetCursorPos(&mut point) {
                eprintln!("GetCursorPos failed: {error}");
                DestroyMenu(menu).ok();
                return;
            }

            let _ = SetForegroundWindow(self.hwnd);
            let _ = TrackPopupMenu(
                menu,
                TPM_LEFTALIGN | TPM_RIGHTBUTTON,
                point.x,
                point.y,
                0,
                self.hwnd,
                None,
            );
            DestroyMenu(menu).ok();
        }
    }

    fn toggle_startup(&self) {
        if is_startup_enabled() {
            match disable_startup() {
                Ok(_) => self.update_status("Run at startup disabled"),
                Err(error) => eprintln!("disable startup failed: {error}"),
            }
        } else {
            match enable_startup() {
                Ok(_) => self.update_status("Run at startup enabled"),
                Err(error) => eprintln!("enable startup failed: {error}"),
            }
        }
    }

    fn cleanup(&mut self) {
        unsafe {
            if let Some(hwnd) = self.status_popup.take() {
                DestroyWindow(hwnd).ok();
            }
            UnregisterHotKey(self.hwnd, HOTKEY_CAPTURE).ok();
            UnregisterHotKey(self.hwnd, HOTKEY_CYCLE_LANGUAGE).ok();
            if self.tray_added {
                let data = notify_icon_data(self.hwnd, "");
                let _ = Shell_NotifyIconW(NIM_DELETE, &data);
                self.tray_added = false;
            }
        }
    }
}

struct OcrWorker {
    tx: Sender<OcrCommand>,
}

impl OcrWorker {
    fn start() -> Self {
        let (tx, rx) = mpsc::channel::<OcrCommand>();
        thread::spawn(move || {
            let mut com_initialized = false;
            let mut controller: Option<OcrController> = None;
            let mut desired_index = 0usize;

            for command in rx {
                match command {
                    OcrCommand::SetLanguage {
                        hwnd_raw,
                        index,
                        warm_after,
                    } => {
                        desired_index = index;
                        if let Some(ocr) = controller.as_mut() {
                            ocr.set_current_index(index);
                        }
                        if warm_after {
                            if let Some(ocr) = ensure_worker_controller(
                                &mut controller,
                                &mut com_initialized,
                                desired_index,
                            ) {
                                worker_warm_ocr(hwnd_raw, ocr, "language-cycle");
                            } else {
                                post_worker_status(hwnd_raw, "OCR warmup failed".to_string());
                            }
                        }
                    }
                    OcrCommand::WarmCurrent { hwnd_raw } => {
                        if let Some(ocr) = ensure_worker_controller(
                            &mut controller,
                            &mut com_initialized,
                            desired_index,
                        ) {
                            worker_warm_ocr(hwnd_raw, ocr, "startup");
                        } else {
                            post_worker_status(hwnd_raw, "OCR warmup failed".to_string());
                        }
                    }
                    OcrCommand::Recognize { hwnd_raw, selected } => {
                        if let Some(ocr) = ensure_worker_controller(
                            &mut controller,
                            &mut com_initialized,
                            desired_index,
                        ) {
                            worker_recognize(hwnd_raw, ocr, selected);
                        } else {
                            post_boxed(
                                hwnd_raw,
                                WM_OCR_DONE,
                                OcrWorkerResult::failure(selected, "OCR worker unavailable"),
                            );
                        }
                    }
                    OcrCommand::Shutdown => break,
                }
            }

            if com_initialized {
                unsafe {
                    CoUninitialize();
                }
            }
        });

        Self { tx }
    }

    fn warm_current(&self, hwnd: HWND) -> std::result::Result<(), mpsc::SendError<OcrCommand>> {
        self.tx.send(OcrCommand::WarmCurrent {
            hwnd_raw: hwnd.0 as isize,
        })
    }

    fn set_language(
        &self,
        hwnd: HWND,
        index: usize,
        warm_after: bool,
    ) -> std::result::Result<(), mpsc::SendError<OcrCommand>> {
        self.tx.send(OcrCommand::SetLanguage {
            hwnd_raw: hwnd.0 as isize,
            index,
            warm_after,
        })
    }

    fn recognize(
        &self,
        hwnd: HWND,
        selected: SelectedImage,
    ) -> std::result::Result<(), mpsc::SendError<OcrCommand>> {
        self.tx.send(OcrCommand::Recognize {
            hwnd_raw: hwnd.0 as isize,
            selected,
        })
    }
}

impl Drop for OcrWorker {
    fn drop(&mut self) {
        let _ = self.tx.send(OcrCommand::Shutdown);
    }
}

enum OcrCommand {
    SetLanguage {
        hwnd_raw: isize,
        index: usize,
        warm_after: bool,
    },
    WarmCurrent {
        hwnd_raw: isize,
    },
    Recognize {
        hwnd_raw: isize,
        selected: SelectedImage,
    },
    Shutdown,
}

struct CaptureWorkerResult {
    result: std::result::Result<CapturedScreen, String>,
}

struct CapturedScreen {
    capture: ScreenCapture,
    capture_ms: f64,
}

struct OcrWorkerResult {
    screen_rect: RectI,
    width: i32,
    height: i32,
    capture_ms: f64,
    result: std::result::Result<OcrWorkerSuccess, String>,
}

impl OcrWorkerResult {
    fn failure(selected: SelectedImage, error: impl Into<String>) -> Self {
        Self {
            screen_rect: selected.screen_rect,
            width: selected.width,
            height: selected.height,
            capture_ms: selected.capture_ms,
            result: Err(error.into()),
        }
    }
}

struct OcrWorkerSuccess {
    outcome: OcrTiming,
    clipboard_ms: f64,
    clipboard_error: Option<String>,
}

fn spawn_screen_capture(hwnd: HWND) {
    let hwnd_raw = hwnd.0 as isize;
    thread::spawn(move || {
        let capture_start = Instant::now();
        let result = capture_virtual_screen()
            .map(|capture| CapturedScreen {
                capture,
                capture_ms: elapsed_ms(capture_start),
            })
            .map_err(|error| error.to_string());
        post_boxed(
            hwnd_raw,
            WM_SCREEN_CAPTURE_DONE,
            CaptureWorkerResult { result },
        );
    });
}

fn ensure_worker_controller<'a>(
    controller: &'a mut Option<OcrController>,
    com_initialized: &mut bool,
    desired_index: usize,
) -> Option<&'a mut OcrController> {
    if !*com_initialized {
        match unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) }.ok() {
            Ok(_) => *com_initialized = true,
            Err(error) => {
                eprintln!("OCR worker COM initialization failed: {error}");
                return None;
            }
        }
    }

    if controller.is_none() {
        match OcrController::new() {
            Ok(mut ocr) => {
                ocr.set_current_index(desired_index);
                *controller = Some(ocr);
            }
            Err(error) => {
                eprintln!("OCR worker initialization failed: {error}");
                return None;
            }
        }
    }

    let ocr = controller.as_mut().expect("controller initialized above");
    ocr.set_current_index(desired_index);
    Some(ocr)
}

fn worker_warm_ocr(hwnd_raw: isize, ocr: &mut OcrController, context: &str) {
    let label = ocr.current_label();
    match ocr.warmup() {
        Ok(timing) => {
            println!(
                "warmup ({context}): language=\"{}\" convert={:.2}ms engine={:.2}ms ocr={:.2}ms total={:.2}ms",
                label, timing.convert_ms, timing.engine_ms, timing.ocr_ms, timing.total_ms
            );
            print_memory("after-ocr-warmup");
            post_worker_status(hwnd_raw, format!("OCR warmed: {label}"));
        }
        Err(error) => {
            eprintln!("OCR warmup failed: {error}");
            post_worker_status(hwnd_raw, "OCR warmup failed".to_string());
        }
    }
}

fn worker_recognize(hwnd_raw: isize, ocr: &mut OcrController, selected: SelectedImage) {
    match ocr.recognize_bgra(selected.width, selected.height, &selected.bgra) {
        Ok(outcome) => {
            if outcome.text.trim().is_empty() {
                post_boxed(
                    hwnd_raw,
                    WM_OCR_DONE,
                    OcrWorkerResult::failure(selected, "No text found"),
                );
                return;
            }

            let clip_start = Instant::now();
            let clipboard_error =
                copy_text_to_clipboard(HWND(hwnd_raw as *mut c_void), &outcome.text)
                    .err()
                    .map(|error| error.to_string());
            let clipboard_ms = elapsed_ms(clip_start);
            post_boxed(
                hwnd_raw,
                WM_OCR_DONE,
                OcrWorkerResult {
                    screen_rect: selected.screen_rect,
                    width: selected.width,
                    height: selected.height,
                    capture_ms: selected.capture_ms,
                    result: Ok(OcrWorkerSuccess {
                        outcome,
                        clipboard_ms,
                        clipboard_error,
                    }),
                },
            );
        }
        Err(error) => {
            post_boxed(
                hwnd_raw,
                WM_OCR_DONE,
                OcrWorkerResult::failure(selected, error.to_string()),
            );
        }
    }
}

fn post_worker_status(hwnd_raw: isize, status: String) {
    post_boxed(hwnd_raw, WM_WORKER_STATUS, status);
}

fn post_boxed<T>(hwnd_raw: isize, msg: u32, payload: T) {
    let ptr = Box::into_raw(Box::new(payload));
    unsafe {
        if let Err(error) = PostMessageW(
            HWND(hwnd_raw as *mut c_void),
            msg,
            WPARAM(ptr as usize),
            LPARAM(0),
        ) {
            eprintln!("PostMessageW({msg}) failed: {error}");
            drop(Box::from_raw(ptr));
        }
    }
}

unsafe extern "system" fn hidden_wnd_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    if msg == WM_NCCREATE {
        let create = &*(lparam.0 as *const CREATESTRUCTW);
        SetWindowLongPtrW(hwnd, GWLP_USERDATA, create.lpCreateParams as isize);
        return LRESULT(1);
    }

    let app_ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut AppState;
    match msg {
        WM_HOTKEY => {
            if let Some(app) = app_ptr.as_mut() {
                match wparam.0 as i32 {
                    HOTKEY_CAPTURE => app.start_capture(),
                    HOTKEY_CYCLE_LANGUAGE => app.cycle_language(),
                    _ => {}
                }
            }
            LRESULT(0)
        }
        WM_TRAY => {
            if let Some(app) = app_ptr.as_mut() {
                match lparam.0 as u32 {
                    WM_RBUTTONUP => app.show_menu(),
                    _ => {}
                }
            }
            LRESULT(0)
        }
        WM_COMMAND => {
            if let Some(app) = app_ptr.as_mut() {
                match loword(wparam.0) as usize {
                    IDM_CAPTURE => app.start_capture(),
                    IDM_CYCLE_LANGUAGE => app.cycle_language(),
                    IDM_LIST_LANGUAGES => print_languages(&app.ocr),
                    IDM_TOGGLE_STARTUP => app.toggle_startup(),
                    IDM_EXIT => {
                        DestroyWindow(hwnd).ok();
                    }
                    id if id >= IDM_LANGUAGE_BASE && id < IDM_LANGUAGE_BASE + 100 => {
                        app.set_language_index(id - IDM_LANGUAGE_BASE);
                    }
                    _ => {}
                }
            }
            LRESULT(0)
        }
        WM_SCREEN_CAPTURE_DONE => {
            if wparam.0 != 0 {
                let message = *Box::from_raw(wparam.0 as *mut CaptureWorkerResult);
                if let Some(app) = app_ptr.as_mut() {
                    app.handle_screen_capture_result(message);
                }
            }
            LRESULT(0)
        }
        WM_SELECTION_DONE => {
            if wparam.0 != 0 {
                let selected = Box::from_raw(wparam.0 as *mut SelectedImage);
                if let Some(app) = app_ptr.as_mut() {
                    app.handle_selected_image(*selected);
                }
            }
            LRESULT(0)
        }
        WM_OCR_DONE => {
            if wparam.0 != 0 {
                let message = *Box::from_raw(wparam.0 as *mut OcrWorkerResult);
                if let Some(app) = app_ptr.as_mut() {
                    app.handle_ocr_result(message);
                }
            }
            LRESULT(0)
        }
        WM_WORKER_STATUS => {
            if wparam.0 != 0 {
                let status = *Box::from_raw(wparam.0 as *mut String);
                if let Some(app) = app_ptr.as_mut() {
                    app.show_status_popup(&status);
                } else {
                    println!("status: {status}");
                }
            }
            LRESULT(0)
        }
        WM_TIMER => {
            if wparam.0 == TIMER_IDLE_METRICS {
                KillTimer(hwnd, TIMER_IDLE_METRICS).ok();
                print_memory("after-idle");
            }
            LRESULT(0)
        }
        WM_DESTROY => {
            PostQuitMessage(0);
            LRESULT(0)
        }
        WM_NCDESTROY => {
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
            DefWindowProcW(hwnd, msg, wparam, lparam)
        }
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}

unsafe extern "system" fn status_wnd_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    if msg == WM_NCCREATE {
        let create = &*(lparam.0 as *const CREATESTRUCTW);
        SetWindowLongPtrW(hwnd, GWLP_USERDATA, create.lpCreateParams as isize);
        SetTimer(hwnd, TIMER_STATUS_HIDE, 2_000, None);
        return LRESULT(1);
    }

    let state_ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut StatusState;
    match msg {
        WM_PAINT => {
            if let Some(state) = state_ptr.as_ref() {
                paint_status_popup(hwnd, state);
            }
            LRESULT(0)
        }
        WM_TIMER => {
            if wparam.0 == TIMER_STATUS_HIDE {
                KillTimer(hwnd, TIMER_STATUS_HIDE).ok();
                DestroyWindow(hwnd).ok();
                return LRESULT(0);
            }
            DefWindowProcW(hwnd, msg, wparam, lparam)
        }
        WM_NCDESTROY => {
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
            if !state_ptr.is_null() {
                drop(Box::from_raw(state_ptr));
            }
            LRESULT(0)
        }
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}

unsafe extern "system" fn overlay_wnd_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    if msg == WM_NCCREATE {
        let create = &*(lparam.0 as *const CREATESTRUCTW);
        SetWindowLongPtrW(hwnd, GWLP_USERDATA, create.lpCreateParams as isize);
        return LRESULT(1);
    }

    let overlay_ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut OverlayState;
    let Some(overlay) = overlay_ptr.as_mut() else {
        return DefWindowProcW(hwnd, msg, wparam, lparam);
    };

    match msg {
        WM_PAINT => {
            paint_overlay(hwnd, overlay);
            LRESULT(0)
        }
        WM_LBUTTONDOWN => {
            let point = point_from_lparam(lparam);
            overlay.dragging = true;
            overlay.start = point;
            overlay.current = point;
            SetCapture(hwnd);
            let _ = InvalidateRect(hwnd, None, BOOL(0));
            LRESULT(0)
        }
        WM_MOUSEMOVE => {
            if overlay.dragging {
                overlay.current = point_from_lparam(lparam);
                let _ = InvalidateRect(hwnd, None, BOOL(0));
            }
            LRESULT(0)
        }
        WM_LBUTTONUP => {
            if overlay.dragging {
                overlay.dragging = false;
                ReleaseCapture().ok();
                overlay.current = point_from_lparam(lparam);
                finish_overlay_selection(hwnd, overlay);
            }
            LRESULT(0)
        }
        WM_RBUTTONDOWN | WM_KEYDOWN => {
            if msg == WM_RBUTTONDOWN || wparam.0 == VK_ESCAPE.0 as usize {
                ReleaseCapture().ok();
                DestroyWindow(hwnd).ok();
                return LRESULT(0);
            }
            DefWindowProcW(hwnd, msg, wparam, lparam)
        }
        WM_NCDESTROY => {
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
            drop(Box::from_raw(overlay_ptr));
            LRESULT(0)
        }
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}

unsafe fn register_window_class(
    class_name: &str,
    wnd_proc: windows::Win32::UI::WindowsAndMessaging::WNDPROC,
    cursor: PCWSTR,
) -> WinResult<()> {
    let class_name = wide_null(class_name);
    let wnd_class = WNDCLASSW {
        lpfnWndProc: wnd_proc,
        hCursor: LoadCursorW(null_hinstance(), cursor).unwrap_or_default(),
        hbrBackground: null_hbrush(),
        lpszClassName: PCWSTR(class_name.as_ptr()),
        ..Default::default()
    };
    let atom = RegisterClassW(&wnd_class);
    if atom == 0 {
        let error = WinError::from_win32();
        let already_exists = error.code().0 as u32 == 0x80070582;
        if !already_exists {
            return Err(error);
        }
    }
    Ok(())
}

unsafe fn create_hidden_window(app: *mut AppState) -> WinResult<HWND> {
    let class_name = wide_null("InstOcrRustHiddenWindow");
    let title = wide_null("InstOCR Rust Native");
    CreateWindowExW(
        WINDOW_EX_STYLE(0),
        PCWSTR(class_name.as_ptr()),
        PCWSTR(title.as_ptr()),
        WINDOW_STYLE(0),
        0,
        0,
        0,
        0,
        null_hwnd(),
        null_hmenu(),
        null_hinstance(),
        Some(app as *const c_void),
    )
}

unsafe fn show_overlay(
    main_hwnd: HWND,
    capture: ScreenCapture,
    capture_ms: f64,
) -> WinResult<HWND> {
    let class_name = wide_null("InstOcrRustSelectionOverlay");
    let title = wide_null("InstOCR Rust Selection");
    let x = capture.x;
    let y = capture.y;
    let width = capture.width;
    let height = capture.height;
    let state = Box::new(OverlayState {
        main_hwnd,
        capture,
        capture_ms,
        dragging: false,
        start: PointI::default(),
        current: PointI::default(),
    });
    let state_ptr = Box::into_raw(state);
    let hwnd = match CreateWindowExW(
        WINDOW_EX_STYLE(WS_EX_TOPMOST.0 | WS_EX_TOOLWINDOW.0),
        PCWSTR(class_name.as_ptr()),
        PCWSTR(title.as_ptr()),
        WS_POPUP,
        x,
        y,
        width,
        height,
        null_hwnd(),
        null_hmenu(),
        null_hinstance(),
        Some(state_ptr as *const c_void),
    ) {
        Ok(hwnd) => hwnd,
        Err(error) => {
            drop(Box::from_raw(state_ptr));
            return Err(error);
        }
    };
    let _ = ShowWindow(hwnd, SW_SHOW);
    let _ = SetForegroundWindow(hwnd);
    Ok(hwnd)
}

unsafe fn show_status_popup(owner: HWND, text: &str) -> WinResult<HWND> {
    let class_name = wide_null("InstOcrRustStatusPopup");
    let title = wide_null("InstOCR Rust Status");
    let width = 420;
    let height = 52;
    let x = GetSystemMetrics(SM_XVIRTUALSCREEN) + GetSystemMetrics(SM_CXVIRTUALSCREEN) - width - 24;
    let y =
        GetSystemMetrics(SM_YVIRTUALSCREEN) + GetSystemMetrics(SM_CYVIRTUALSCREEN) - height - 48;
    let state = Box::new(StatusState {
        text: text.to_string(),
    });
    let state_ptr = Box::into_raw(state);
    let hwnd = match CreateWindowExW(
        WINDOW_EX_STYLE(WS_EX_TOPMOST.0 | WS_EX_TOOLWINDOW.0 | WS_EX_NOACTIVATE.0),
        PCWSTR(class_name.as_ptr()),
        PCWSTR(title.as_ptr()),
        WS_POPUP,
        x,
        y,
        width,
        height,
        owner,
        null_hmenu(),
        null_hinstance(),
        Some(state_ptr as *const c_void),
    ) {
        Ok(hwnd) => hwnd,
        Err(error) => {
            drop(Box::from_raw(state_ptr));
            return Err(error);
        }
    };
    let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
    Ok(hwnd)
}

unsafe fn paint_status_popup(hwnd: HWND, state: &StatusState) {
    let mut paint = PAINTSTRUCT::default();
    let hdc = BeginPaint(hwnd, &mut paint);
    let brush = CreateSolidBrush(COLORREF(0x202020));
    let old_brush = SelectObject(hdc, brush);
    let old_pen = SelectObject(hdc, GetStockObject(HOLLOW_BRUSH));
    let _ = Rectangle(hdc, 0, 0, 420, 52);
    SelectObject(hdc, old_pen);
    SelectObject(hdc, old_brush);
    let _ = DeleteObject(brush);

    let _ = SetBkMode(hdc, TRANSPARENT);
    let _ = SetTextColor(hdc, COLORREF(0x00ffffff));
    let wide = state.text.encode_utf16().collect::<Vec<u16>>();
    let _ = TextOutW(hdc, 14, 17, &wide);
    let _ = EndPaint(hwnd, &paint);
}

fn notify_icon_data(hwnd: HWND, tip: &str) -> NOTIFYICONDATAW {
    let mut data = NOTIFYICONDATAW::default();
    data.cbSize = size_of::<NOTIFYICONDATAW>() as u32;
    data.hWnd = hwnd;
    data.uID = TRAY_ID;
    copy_wide_fixed(&mut data.szTip, tip);
    data
}

unsafe fn append_menu(menu: HMENU, id: usize, text: &str) {
    let wide = wide_null(text);
    AppendMenuW(menu, MF_STRING, id, PCWSTR(wide.as_ptr())).ok();
}

unsafe fn append_language_menu(menu: HMENU, ocr: &OcrController) {
    let auto_label = if ocr.current_index() == 0 {
        "OCR: Auto ✓"
    } else {
        "OCR: Auto"
    };
    let wide = wide_null(auto_label);
    let flags = MF_STRING
        | if ocr.current_index() == 0 {
            MF_CHECKED
        } else {
            MF_UNCHECKED
        };
    AppendMenuW(menu, flags, IDM_LANGUAGE_BASE, PCWSTR(wide.as_ptr())).ok();

    for (index, language) in ocr.installed.iter().enumerate() {
        let label = if ocr.current_index() == index + 1 {
            format!("OCR: {} ({}) ✓", language.display_name, language.tag)
        } else {
            format!("OCR: {} ({})", language.display_name, language.tag)
        };
        let wide = wide_null(&label);
        let flags = MF_STRING
            | if ocr.current_index() == index + 1 {
                MF_CHECKED
            } else {
                MF_UNCHECKED
            };
        AppendMenuW(
            menu,
            flags,
            IDM_LANGUAGE_BASE + index + 1,
            PCWSTR(wide.as_ptr()),
        )
        .ok();
    }
}

struct OcrController {
    installed: Vec<LanguageInfo>,
    current_index: usize,
    auto_engine: Option<OcrEngine>,
    language_engine: Option<(usize, OcrEngine)>,
}

impl OcrController {
    fn new() -> WinResult<Self> {
        let languages = OcrEngine::AvailableRecognizerLanguages()?;
        let mut installed = Vec::new();
        for index in 0..languages.Size()? {
            let language = languages.GetAt(index)?;
            let tag = language.LanguageTag()?.to_string();
            let display_name = language.DisplayName()?.to_string();
            installed.push(LanguageInfo {
                language,
                tag,
                display_name,
            });
        }
        installed.sort_by(|left, right| left.tag.cmp(&right.tag));

        Ok(Self {
            installed,
            current_index: 0,
            auto_engine: None,
            language_engine: None,
        })
    }

    fn current_label(&self) -> String {
        if self.current_index == 0 {
            "Auto (user profile languages)".to_string()
        } else {
            self.installed
                .get(self.current_index - 1)
                .map(|language| format!("{} ({})", language.display_name, language.tag))
                .unwrap_or_else(|| "Unknown".to_string())
        }
    }

    fn current_index(&self) -> usize {
        self.current_index
    }

    fn current_language_tag(&self) -> Option<String> {
        if self.current_index == 0 {
            None
        } else {
            self.installed
                .get(self.current_index - 1)
                .map(|language| language.tag.clone())
        }
    }

    fn set_current_index(&mut self, index: usize) {
        let count = self.installed.len() + 1;
        self.current_index = index % count.max(1);
    }

    fn set_current_language_tag(&mut self, language_tag: Option<&str>) {
        let Some(language_tag) = language_tag else {
            self.current_index = 0;
            return;
        };

        self.current_index = self
            .installed
            .iter()
            .position(|language| language.tag.eq_ignore_ascii_case(language_tag))
            .map(|index| index + 1)
            .unwrap_or(0);
    }

    fn cycle_language(&mut self) -> String {
        let count = self.installed.len() + 1;
        self.current_index = (self.current_index + 1) % count.max(1);
        self.current_label()
    }

    fn warmup(&mut self) -> WinResult<OcrTiming> {
        let bgra = vec![255u8; 64 * 64 * 4];
        self.recognize_bgra(64, 64, &bgra)
    }

    fn recognize_bgra(&mut self, width: i32, height: i32, bgra: &[u8]) -> WinResult<OcrTiming> {
        let total_start = Instant::now();

        let convert_start = Instant::now();
        let preprocessed = preprocess_bgra(width, height, bgra)?;
        let bitmap =
            software_bitmap_from_bgra(preprocessed.width, preprocessed.height, &preprocessed.bgra)?;
        let convert_ms = elapsed_ms(convert_start);

        let engine_start = Instant::now();
        let engine = self.current_engine()?;
        let engine_ms = elapsed_ms(engine_start);

        let ocr_start = Instant::now();
        let result = engine.RecognizeAsync(&bitmap)?.get()?;
        let ocr_ms = elapsed_ms(ocr_start);
        let text = format_ocr_result(&result, engine)?;

        Ok(OcrTiming {
            text,
            convert_ms,
            engine_ms,
            ocr_ms,
            total_ms: elapsed_ms(total_start),
        })
    }

    fn current_engine(&mut self) -> WinResult<&OcrEngine> {
        if self.current_index == 0 {
            if self.auto_engine.is_none() {
                self.auto_engine = Some(OcrEngine::TryCreateFromUserProfileLanguages()?);
            }
            return Ok(self.auto_engine.as_ref().expect("auto engine just created"));
        }

        let language_index = self.current_index - 1;
        let needs_new = self
            .language_engine
            .as_ref()
            .map(|(index, _)| *index != language_index)
            .unwrap_or(true);
        if needs_new {
            let engine =
                OcrEngine::TryCreateFromLanguage(&self.installed[language_index].language)?;
            self.language_engine = Some((language_index, engine));
        }
        Ok(&self
            .language_engine
            .as_ref()
            .expect("language engine just created")
            .1)
    }
}

struct LanguageInfo {
    language: Language,
    tag: String,
    display_name: String,
}

struct OcrTiming {
    text: String,
    convert_ms: f64,
    engine_ms: f64,
    ocr_ms: f64,
    total_ms: f64,
}

fn print_languages(ocr: &OcrController) {
    println!("OCR language modes:");
    println!("  Auto: TryCreateFromUserProfileLanguages");
    for language in &ocr.installed {
        println!("  {} - {}", language.tag, language.display_name);
    }
    println!("Active OCR language: {}", ocr.current_label());
}

struct PreprocessedImage {
    width: i32,
    height: i32,
    bgra: Vec<u8>,
}

fn preprocess_bgra(width: i32, height: i32, bgra: &[u8]) -> WinResult<PreprocessedImage> {
    let expected_len = width as usize * height as usize * 4;
    if width <= 0 || height <= 0 || bgra.len() != expected_len {
        return Err(WinError::new(
            windows::core::HRESULT(0x80070057u32 as i32),
            "BGRA buffer length does not match dimensions",
        ));
    }

    let min_dimension = 64;
    let padding = 8;
    let requires_padding = width < min_dimension || height < min_dimension;
    let padded_width = if requires_padding {
        (width + padding * 2).max(min_dimension + padding * 2)
    } else {
        width
    };
    let padded_height = if requires_padding {
        (height + padding * 2).max(min_dimension + padding * 2)
    } else {
        height
    };

    let mut current = if requires_padding {
        let mut padded = vec![0u8; padded_width as usize * padded_height as usize * 4];
        let corner = &bgra[0..4];
        for pixel in padded.chunks_exact_mut(4) {
            pixel.copy_from_slice(corner);
        }

        for row in 0..height as usize {
            let src_offset = row * width as usize * 4;
            let dst_offset =
                ((row + padding as usize) * padded_width as usize + padding as usize) * 4;
            let row_bytes = width as usize * 4;
            padded[dst_offset..dst_offset + row_bytes]
                .copy_from_slice(&bgra[src_offset..src_offset + row_bytes]);
        }

        PreprocessedImage {
            width: padded_width,
            height: padded_height,
            bgra: padded,
        }
    } else {
        PreprocessedImage {
            width,
            height,
            bgra: bgra.to_vec(),
        }
    };

    let max_dimension = OcrEngine::MaxImageDimension()? as i32;
    let max_padded = current.width.max(current.height);
    let scale = if max_padded > max_dimension {
        max_dimension as f64 / max_padded as f64
    } else if max_padded <= 1000
        && (current.width as f64 * 1.5) <= max_dimension as f64
        && (current.height as f64 * 1.5) <= max_dimension as f64
    {
        1.5
    } else {
        1.0
    };

    let output_width = ((current.width as f64 * scale).round() as i32).clamp(1, max_dimension);
    let output_height = ((current.height as f64 * scale).round() as i32).clamp(1, max_dimension);

    if output_width != current.width || output_height != current.height {
        current.bgra = if scale >= 1.0 {
            resize_bgra_bicubic(
                &current.bgra,
                current.width,
                current.height,
                output_width,
                output_height,
            )
        } else {
            resize_bgra_bilinear(
                &current.bgra,
                current.width,
                current.height,
                output_width,
                output_height,
            )
        };
        current.width = output_width;
        current.height = output_height;
    }

    Ok(current)
}

fn resize_bgra_bilinear(
    src: &[u8],
    src_width: i32,
    src_height: i32,
    dst_width: i32,
    dst_height: i32,
) -> Vec<u8> {
    let mut dst = vec![0u8; dst_width as usize * dst_height as usize * 4];
    let scale_x = src_width as f64 / dst_width as f64;
    let scale_y = src_height as f64 / dst_height as f64;

    for y in 0..dst_height {
        let src_y = ((y as f64 + 0.5) * scale_y - 0.5).clamp(0.0, (src_height - 1) as f64);
        let y0 = src_y.floor() as i32;
        let y1 = (y0 + 1).min(src_height - 1);
        let wy = src_y - y0 as f64;

        for x in 0..dst_width {
            let src_x = ((x as f64 + 0.5) * scale_x - 0.5).clamp(0.0, (src_width - 1) as f64);
            let x0 = src_x.floor() as i32;
            let x1 = (x0 + 1).min(src_width - 1);
            let wx = src_x - x0 as f64;

            for channel in 0..4usize {
                let p00 = src[((y0 * src_width + x0) as usize * 4) + channel] as f64;
                let p10 = src[((y0 * src_width + x1) as usize * 4) + channel] as f64;
                let p01 = src[((y1 * src_width + x0) as usize * 4) + channel] as f64;
                let p11 = src[((y1 * src_width + x1) as usize * 4) + channel] as f64;
                let top = p00 + (p10 - p00) * wx;
                let bottom = p01 + (p11 - p01) * wx;
                let value = top + (bottom - top) * wy;
                dst[((y * dst_width + x) as usize * 4) + channel] =
                    value.round().clamp(0.0, 255.0) as u8;
            }
        }
    }

    dst
}

fn resize_bgra_bicubic(
    src: &[u8],
    src_width: i32,
    src_height: i32,
    dst_width: i32,
    dst_height: i32,
) -> Vec<u8> {
    let mut dst = vec![0u8; dst_width as usize * dst_height as usize * 4];
    let scale_x = src_width as f64 / dst_width as f64;
    let scale_y = src_height as f64 / dst_height as f64;

    for y in 0..dst_height {
        let src_y = (y as f64 + 0.5) * scale_y - 0.5;
        let y_int = src_y.floor() as i32;
        let y_frac = src_y - y_int as f64;

        for x in 0..dst_width {
            let src_x = (x as f64 + 0.5) * scale_x - 0.5;
            let x_int = src_x.floor() as i32;
            let x_frac = src_x - x_int as f64;

            for channel in 0..4usize {
                let mut value = 0.0;
                for m in -1..=2 {
                    let sample_y = (y_int + m).clamp(0, src_height - 1);
                    let weight_y = cubic_weight(m as f64 - y_frac);
                    for n in -1..=2 {
                        let sample_x = (x_int + n).clamp(0, src_width - 1);
                        let weight_x = cubic_weight(n as f64 - x_frac);
                        let sample =
                            src[((sample_y * src_width + sample_x) as usize * 4) + channel] as f64;
                        value += sample * weight_x * weight_y;
                    }
                }
                dst[((y * dst_width + x) as usize * 4) + channel] =
                    value.round().clamp(0.0, 255.0) as u8;
            }
        }
    }

    dst
}

fn cubic_weight(x: f64) -> f64 {
    let a = -0.5;
    let x = x.abs();
    if x <= 1.0 {
        (a + 2.0) * x.powi(3) - (a + 3.0) * x.powi(2) + 1.0
    } else if x < 2.0 {
        a * x.powi(3) - 5.0 * a * x.powi(2) + 8.0 * a * x - 4.0 * a
    } else {
        0.0
    }
}

fn format_ocr_result(result: &OcrResult, engine: &OcrEngine) -> WinResult<String> {
    let language_tag = engine.RecognizerLanguage()?.LanguageTag()?.to_string();
    let lines = result.Lines()?;
    let mut output = Vec::new();

    for index in 0..lines.Size()? {
        let line = lines.GetAt(index)?;
        let text = if is_no_space_language(&language_tag) {
            let words = line.Words()?;
            let mut joined = String::new();
            let mut previous_space_joining = false;
            for word_index in 0..words.Size()? {
                let word = words.GetAt(word_index)?.Text()?.to_string();
                let current_space_joining = word_usually_separated_by_space(&word);
                if !joined.is_empty() && (current_space_joining || previous_space_joining) {
                    joined.push(' ');
                }
                joined.push_str(&word);
                previous_space_joining = current_space_joining;
            }
            joined
        } else {
            line.Text()?.to_string()
        };

        let text = if is_right_to_left(&language_tag) {
            reverse_words(&text)
        } else {
            text
        };

        if !text.trim().is_empty() {
            output.push(text.trim().to_string());
        }
    }

    Ok(output.join("\r\n"))
}

fn is_no_space_language(language_tag: &str) -> bool {
    language_tag.starts_with("zh") || language_tag.starts_with("ja")
}

fn is_right_to_left(language_tag: &str) -> bool {
    ["ar", "fa", "he", "ur"]
        .iter()
        .any(|prefix| language_tag.starts_with(prefix))
}

fn reverse_words(text: &str) -> String {
    let mut words = text.split_whitespace().collect::<Vec<_>>();
    words.reverse();
    words.join(" ")
}

fn word_usually_separated_by_space(word: &str) -> bool {
    if word.is_empty() {
        return false;
    }

    if word.chars().count() > 1 {
        return true;
    }

    let ch = word.chars().next().expect("word is not empty");
    ch.is_ascii_alphanumeric()
}

fn software_bitmap_from_bgra(width: i32, height: i32, bgra: &[u8]) -> WinResult<SoftwareBitmap> {
    let expected_len = width as usize * height as usize * 4;
    if bgra.len() != expected_len {
        return Err(WinError::new(
            windows::core::HRESULT(0x80070057u32 as i32),
            "BGRA buffer length does not match dimensions",
        ));
    }

    let writer = DataWriter::new()?;
    writer.WriteBytes(bgra)?;
    let buffer = writer.DetachBuffer()?;
    SoftwareBitmap::CreateCopyFromBuffer(&buffer, BitmapPixelFormat::Bgra8, width, height)
}

struct ScreenCapture {
    x: i32,
    y: i32,
    width: i32,
    height: i32,
    bgra: Vec<u8>,
}

struct SelectedImage {
    width: i32,
    height: i32,
    bgra: Vec<u8>,
    screen_rect: RectI,
    capture_ms: f64,
}

struct OverlayState {
    main_hwnd: HWND,
    capture: ScreenCapture,
    capture_ms: f64,
    dragging: bool,
    start: PointI,
    current: PointI,
}

struct StatusState {
    text: String,
}

#[derive(Clone, Copy, Default)]
struct PointI {
    x: i32,
    y: i32,
}

#[derive(Clone, Copy)]
struct RectI {
    left: i32,
    top: i32,
    right: i32,
    bottom: i32,
}

impl RectI {
    fn new(left: i32, top: i32, right: i32, bottom: i32) -> Self {
        Self {
            left: left.min(right),
            top: top.min(bottom),
            right: left.max(right),
            bottom: top.max(bottom),
        }
    }

    fn width(self) -> i32 {
        self.right - self.left
    }

    fn height(self) -> i32 {
        self.bottom - self.top
    }

    fn clamp(self, width: i32, height: i32) -> Self {
        Self {
            left: self.left.clamp(0, width),
            top: self.top.clamp(0, height),
            right: self.right.clamp(0, width),
            bottom: self.bottom.clamp(0, height),
        }
    }
}

fn capture_virtual_screen() -> AnyResult<ScreenCapture> {
    unsafe {
        let x = GetSystemMetrics(SM_XVIRTUALSCREEN);
        let y = GetSystemMetrics(SM_YVIRTUALSCREEN);
        let width = GetSystemMetrics(SM_CXVIRTUALSCREEN);
        let height = GetSystemMetrics(SM_CYVIRTUALSCREEN);
        if width <= 0 || height <= 0 {
            return Err("virtual screen dimensions are invalid".into());
        }

        let screen_dc = GetDC(null_hwnd());
        if screen_dc.0.is_null() {
            return Err("GetDC(NULL) failed".into());
        }

        let result = capture_from_dc(screen_dc, x, y, width, height);
        ReleaseDC(null_hwnd(), screen_dc);
        result
    }
}

unsafe fn capture_from_dc(
    screen_dc: HDC,
    x: i32,
    y: i32,
    width: i32,
    height: i32,
) -> AnyResult<ScreenCapture> {
    let mem_dc = CreateCompatibleDC(screen_dc);
    if mem_dc.0.is_null() {
        return Err("CreateCompatibleDC failed".into());
    }

    let bitmap = CreateCompatibleBitmap(screen_dc, width, height);
    if bitmap.0.is_null() {
        let _ = DeleteDC(mem_dc);
        return Err("CreateCompatibleBitmap failed".into());
    }

    let old = SelectObject(mem_dc, bitmap);
    let bitblt = BitBlt(mem_dc, 0, 0, width, height, screen_dc, x, y, SRCCOPY);
    let mut bgra = vec![0u8; width as usize * height as usize * 4];
    let mut bmi = bitmap_info(width, height);
    let lines = if bitblt.is_ok() {
        GetDIBits(
            mem_dc,
            bitmap,
            0,
            height as u32,
            Some(bgra.as_mut_ptr() as *mut c_void),
            &mut bmi,
            DIB_RGB_COLORS,
        )
    } else {
        0
    };

    SelectObject(mem_dc, old);
    let _ = DeleteObject(bitmap);
    let _ = DeleteDC(mem_dc);

    bitblt?;
    if lines == 0 {
        return Err("GetDIBits failed".into());
    }

    Ok(ScreenCapture {
        x,
        y,
        width,
        height,
        bgra,
    })
}

fn crop_capture(capture: &ScreenCapture, rect: RectI) -> AnyResult<SelectedImage> {
    let rect = rect.clamp(capture.width, capture.height);
    if rect.width() < 3 || rect.height() < 3 {
        return Err("selection is too small".into());
    }

    let mut bgra = vec![0u8; rect.width() as usize * rect.height() as usize * 4];
    let src_stride = capture.width as usize * 4;
    let dst_stride = rect.width() as usize * 4;

    for row in 0..rect.height() as usize {
        let src_offset =
            ((rect.top as usize + row) * capture.width as usize + rect.left as usize) * 4;
        let dst_offset = row * dst_stride;
        bgra[dst_offset..dst_offset + dst_stride]
            .copy_from_slice(&capture.bgra[src_offset..src_offset + dst_stride.min(src_stride)]);
    }

    Ok(SelectedImage {
        width: rect.width(),
        height: rect.height(),
        bgra,
        screen_rect: RectI {
            left: capture.x + rect.left,
            top: capture.y + rect.top,
            right: capture.x + rect.right,
            bottom: capture.y + rect.bottom,
        },
        capture_ms: 0.0,
    })
}

unsafe fn paint_overlay(hwnd: HWND, overlay: &OverlayState) {
    let mut paint = PAINTSTRUCT::default();
    let hdc = BeginPaint(hwnd, &mut paint);
    let bmi = bitmap_info(overlay.capture.width, overlay.capture.height);
    StretchDIBits(
        hdc,
        0,
        0,
        overlay.capture.width,
        overlay.capture.height,
        0,
        0,
        overlay.capture.width,
        overlay.capture.height,
        Some(overlay.capture.bgra.as_ptr() as *const c_void),
        &bmi,
        DIB_RGB_COLORS,
        SRCCOPY,
    );

    if overlay.dragging {
        let rect = RectI::new(
            overlay.start.x,
            overlay.start.y,
            overlay.current.x,
            overlay.current.y,
        )
        .clamp(overlay.capture.width, overlay.capture.height);
        draw_selection_rect(hdc, rect);
    }

    let _ = EndPaint(hwnd, &paint);
}

unsafe fn draw_selection_rect(hdc: HDC, rect: RectI) {
    let pen = CreatePen(PS_SOLID, 2, COLORREF(0x0000ff));
    let old_pen = SelectObject(hdc, pen);
    let old_brush = SelectObject(hdc, GetStockObject(HOLLOW_BRUSH));
    let _ = Rectangle(hdc, rect.left, rect.top, rect.right, rect.bottom);
    SelectObject(hdc, old_brush);
    SelectObject(hdc, old_pen);
    let _ = DeleteObject(pen);
}

unsafe fn finish_overlay_selection(hwnd: HWND, overlay: &mut OverlayState) {
    let rect = RectI::new(
        overlay.start.x,
        overlay.start.y,
        overlay.current.x,
        overlay.current.y,
    )
    .clamp(overlay.capture.width, overlay.capture.height);

    match crop_capture(&overlay.capture, rect) {
        Ok(mut selected) => {
            selected.capture_ms = overlay.capture_ms;
            let ptr = Box::into_raw(Box::new(selected));
            if let Err(error) = PostMessageW(
                overlay.main_hwnd,
                WM_SELECTION_DONE,
                WPARAM(ptr as usize),
                LPARAM(0),
            ) {
                eprintln!("PostMessageW(WM_SELECTION_DONE) failed: {error}");
                drop(Box::from_raw(ptr));
            }
        }
        Err(error) => post_worker_status(
            overlay.main_hwnd.0 as isize,
            format!("Selection ignored: {error}"),
        ),
    }
    DestroyWindow(hwnd).ok();
}

fn bitmap_info(width: i32, height: i32) -> BITMAPINFO {
    BITMAPINFO {
        bmiHeader: BITMAPINFOHEADER {
            biSize: size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: width,
            biHeight: -height,
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB.0,
            biSizeImage: (width * height * 4) as u32,
            ..Default::default()
        },
        ..Default::default()
    }
}

fn copy_text_to_clipboard(hwnd: HWND, text: &str) -> WinResult<()> {
    unsafe {
        OpenClipboard(hwnd)?;
        let clipboard_result = (|| {
            EmptyClipboard()?;
            let mut wide = text.encode_utf16().collect::<Vec<u16>>();
            wide.push(0);
            let byte_len = wide.len() * size_of::<u16>();
            let mem = GlobalAlloc(GMEM_MOVEABLE, byte_len)?;
            let ptr = GlobalLock(mem) as *mut u16;
            if ptr.is_null() {
                return Err(WinError::from_win32());
            }
            std::ptr::copy_nonoverlapping(wide.as_ptr(), ptr, wide.len());
            GlobalUnlock(mem).ok();
            SetClipboardData(CF_UNICODETEXT.0 as u32, HANDLE(mem.0))?;
            Ok(())
        })();
        CloseClipboard().ok();
        clipboard_result
    }
}

fn enable_startup() -> std::io::Result<()> {
    let exe = std::env::current_exe()?;
    let command = format!("\"{}\"", exe.display());
    let mut wide = command.encode_utf16().collect::<Vec<u16>>();
    wide.push(0);
    let bytes = wide
        .iter()
        .flat_map(|unit| unit.to_le_bytes())
        .collect::<Vec<u8>>();

    unsafe {
        let key = open_or_create_run_key(REG_SAM_FLAGS(KEY_SET_VALUE.0))?;
        let name = wide_null(STARTUP_VALUE_NAME);
        let status = RegSetValueExW(key, PCWSTR(name.as_ptr()), 0, REG_SZ, Some(&bytes));
        let _ = RegCloseKey(key);
        win32_status_to_io(status)
    }
}

fn disable_startup() -> std::io::Result<()> {
    unsafe {
        let key = open_run_key(REG_SAM_FLAGS(KEY_SET_VALUE.0))?;
        let name = wide_null(STARTUP_VALUE_NAME);
        let status = RegDeleteValueW(key, PCWSTR(name.as_ptr()));
        let _ = RegCloseKey(key);
        if status == ERROR_SUCCESS {
            Ok(())
        } else {
            Err(std::io::Error::from_raw_os_error(status.0 as i32))
        }
    }
}

fn is_startup_enabled() -> bool {
    unsafe {
        let key = match open_run_key(KEY_READ) {
            Ok(key) => key,
            Err(_) => return false,
        };
        let name = wide_null(STARTUP_VALUE_NAME);
        let mut value_type = REG_SZ;
        let mut byte_len = 0u32;
        let status = RegQueryValueExW(
            key,
            PCWSTR(name.as_ptr()),
            None,
            Some(&mut value_type),
            None,
            Some(&mut byte_len),
        );
        let _ = RegCloseKey(key);
        status == ERROR_SUCCESS && value_type == REG_SZ && byte_len > 0
    }
}

unsafe fn open_or_create_run_key(access: REG_SAM_FLAGS) -> std::io::Result<HKEY> {
    let subkey = wide_null(RUN_KEY);
    let mut key = HKEY::default();
    let status = RegCreateKeyExW(
        HKEY_CURRENT_USER,
        PCWSTR(subkey.as_ptr()),
        0,
        PCWSTR(null()),
        REG_OPTION_NON_VOLATILE,
        access,
        None,
        &mut key,
        None,
    );
    if status == ERROR_SUCCESS {
        Ok(key)
    } else {
        Err(std::io::Error::from_raw_os_error(status.0 as i32))
    }
}

unsafe fn open_run_key(access: REG_SAM_FLAGS) -> std::io::Result<HKEY> {
    let subkey = wide_null(RUN_KEY);
    let mut key = HKEY::default();
    let status = RegOpenKeyExW(
        HKEY_CURRENT_USER,
        PCWSTR(subkey.as_ptr()),
        0,
        access,
        &mut key,
    );
    if status == ERROR_SUCCESS {
        Ok(key)
    } else {
        Err(std::io::Error::from_raw_os_error(status.0 as i32))
    }
}

fn win32_status_to_io(status: windows::Win32::Foundation::WIN32_ERROR) -> std::io::Result<()> {
    if status == ERROR_SUCCESS {
        Ok(())
    } else {
        Err(std::io::Error::from_raw_os_error(status.0 as i32))
    }
}

fn print_memory(label: &str) {
    unsafe {
        let mut counters = PROCESS_MEMORY_COUNTERS_EX::default();
        if GetProcessMemoryInfo(
            GetCurrentProcess(),
            &mut counters as *mut _ as *mut _,
            size_of::<PROCESS_MEMORY_COUNTERS_EX>() as u32,
        )
        .is_ok()
        {
            println!(
                "memory {label}: private_bytes={:.2}MiB working_set={:.2}MiB",
                mib(counters.PrivateUsage),
                mib(counters.WorkingSetSize)
            );
        }
    }
}

fn mib(bytes: usize) -> f64 {
    bytes as f64 / 1024.0 / 1024.0
}

fn elapsed_ms(start: Instant) -> f64 {
    start.elapsed().as_secs_f64() * 1000.0
}

fn wide_null(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(std::iter::once(0)).collect()
}

fn copy_wide_fixed(target: &mut [u16], value: &str) {
    target.fill(0);
    let max_units = target.len().saturating_sub(1);
    for (slot, unit) in target.iter_mut().take(max_units).zip(value.encode_utf16()) {
        *slot = unit;
    }
}

fn null_hwnd() -> HWND {
    HWND(null_mut())
}

fn null_hinstance() -> HINSTANCE {
    HINSTANCE(null_mut())
}

fn null_hmenu() -> HMENU {
    HMENU(null_mut())
}

fn null_hbrush() -> HBRUSH {
    HBRUSH(null_mut())
}

fn point_from_lparam(lparam: LPARAM) -> PointI {
    PointI {
        x: loword_signed(lparam.0),
        y: hiword_signed(lparam.0),
    }
}

fn loword(value: usize) -> u16 {
    (value & 0xffff) as u16
}

fn loword_signed(value: isize) -> i32 {
    (value as u32 & 0xffff) as i16 as i32
}

fn hiword_signed(value: isize) -> i32 {
    ((value as u32 >> 16) & 0xffff) as i16 as i32
}
