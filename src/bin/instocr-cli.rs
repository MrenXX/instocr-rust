use std::error::Error;
use std::ffi::c_void;
use std::fs;
use std::mem::size_of;
use std::path::Path;
use std::ptr::null_mut;
use std::time::{Duration, Instant};

use fontdue::{Font, FontSettings};
use windows::core::{Error as WinError, Result as WinResult};
use windows::Graphics::Imaging::{BitmapPixelFormat, SoftwareBitmap};
use windows::Media::Ocr::{OcrEngine, OcrResult};
use windows::Storage::Streams::DataWriter;
use windows::Win32::Foundation::{HANDLE, HWND};
use windows::Win32::Graphics::Gdi::{
    BitBlt, CreateCompatibleBitmap, CreateCompatibleDC, DeleteDC, DeleteObject, GetDC, GetDIBits,
    ReleaseDC, SelectObject, BITMAPINFO, BITMAPINFOHEADER, BI_RGB, DIB_RGB_COLORS, HDC, SRCCOPY,
};
use windows::Win32::System::Com::{CoInitializeEx, CoUninitialize, COINIT_MULTITHREADED};
use windows::Win32::System::DataExchange::{
    CloseClipboard, EmptyClipboard, OpenClipboard, SetClipboardData,
};
use windows::Win32::System::Memory::{GlobalAlloc, GlobalLock, GlobalUnlock, GMEM_MOVEABLE};
use windows::Win32::System::Ole::CF_UNICODETEXT;
use windows::Win32::System::ProcessStatus::{GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS_EX};
use windows::Win32::System::Threading::GetCurrentProcess;
use windows::Win32::UI::HiDpi::{
    SetProcessDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
};
use windows::Win32::UI::Input::KeyboardAndMouse::{MOD_ALT, MOD_CONTROL, MOD_SHIFT};
use windows::Win32::UI::WindowsAndMessaging::{
    GetSystemMetrics, SM_CXVIRTUALSCREEN, SM_CYVIRTUALSCREEN, SM_XVIRTUALSCREEN, SM_YVIRTUALSCREEN,
};

type AnyResult<T> = std::result::Result<T, Box<dyn Error>>;

fn main() -> AnyResult<()> {
    unsafe {
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        CoInitializeEx(None, COINIT_MULTITHREADED).ok()?;
    }

    let result = run();

    unsafe {
        CoUninitialize();
    }

    result
}

fn run() -> AnyResult<()> {
    let args = std::env::args().skip(1).collect::<Vec<_>>();
    let resident_warm = args.iter().any(|arg| arg == "--resident-warm");
    let command = args
        .iter()
        .find(|arg| arg.as_str() != "--resident-warm")
        .map(String::as_str)
        .unwrap_or("--measure");

    match command {
        "--measure" => run_measurement(parse_idle_seconds(&args).unwrap_or(2), resident_warm),
        "--validate-hotkey" => {
            let index = args
                .iter()
                .position(|arg| arg == "--validate-hotkey")
                .ok_or("--validate-hotkey command missing")?;
            let value = args
                .get(index + 1)
                .ok_or("--validate-hotkey requires a value")?;
            match Hotkey::parse(value) {
                Ok(hotkey) => {
                    println!("{}", hotkey.display);
                    Ok(())
                }
                Err(error) => Err(error.into()),
            }
        }
        "--path-benchmark" => run_path_benchmark(),
        "--list-languages" => {
            let ocr = OcrController::new()?;
            print_languages(&ocr);
            Ok(())
        }
        "--capture-rect" => {
            let index = args
                .iter()
                .position(|arg| arg == "--capture-rect")
                .ok_or("--capture-rect command missing")?;
            run_capture_rect(&args[index + 1..])
        }
        other => {
            eprintln!("Unknown command: {other}");
            eprintln!("Usage:");
            eprintln!("  instocr-cli.exe --measure [--resident-warm] [--idle-seconds N]");
            eprintln!("  instocr-cli.exe --validate-hotkey \"Ctrl+Alt+D\"");
            eprintln!("  instocr-cli.exe --path-benchmark");
            eprintln!("  instocr-cli.exe --list-languages");
            eprintln!("  instocr-cli.exe --capture-rect X Y W H");
            Ok(())
        }
    }
}

#[derive(Clone)]
struct Hotkey {
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

fn parse_idle_seconds(args: &[String]) -> Option<u64> {
    args.windows(2)
        .find(|pair| pair[0] == "--idle-seconds")
        .and_then(|pair| pair[1].parse::<u64>().ok())
}

fn run_measurement(idle_seconds: u64, resident_warm: bool) -> AnyResult<()> {
    println!("InstOCR Rust native CLI measurement");
    println!(
        "mode: {}",
        if resident_warm {
            "resident-warm"
        } else {
            "on-demand"
        }
    );
    print_memory("startup");

    let mut ocr = OcrController::new()?;
    print_languages(&ocr);
    print_memory("after-language-enumeration");

    if resident_warm {
        let warm = ocr.warmup()?;
        println!(
            "warmup: convert={:.2}ms engine={:.2}ms ocr={:.2}ms total={:.2}ms",
            warm.convert_ms, warm.engine_ms, warm.ocr_ms, warm.total_ms
        );
    } else {
        println!("warmup: skipped (on-demand mode)");
    }
    print_memory("after-optional-warm");

    let cycle_start = Instant::now();
    let label = ocr.cycle_language();
    println!(
        "language-cycle: active=\"{}\" elapsed={:.2}ms",
        label,
        elapsed_ms(cycle_start)
    );
    if resident_warm {
        let warm = ocr.warmup()?;
        println!(
            "language-cycle-warmup: convert={:.2}ms engine={:.2}ms ocr={:.2}ms total={:.2}ms",
            warm.convert_ms, warm.engine_ms, warm.ocr_ms, warm.total_ms
        );
    }
    print_memory("after-language-cycle");

    let font = load_font();
    let render_start = Instant::now();
    let bgra = render_case(
        &font,
        900,
        300,
        "InstOCR native Rust measurement text for Windows OCR.",
    );
    let render_ms = elapsed_ms(render_start);
    let outcome = ocr.recognize_bgra(900, 300, &bgra)?;
    println!(
        "synthetic-capture-ocr: capture={:.2}ms convert={:.2}ms engine={:.2}ms ocr={:.2}ms total={:.2}ms text=\"{}\"",
        render_ms,
        outcome.convert_ms,
        outcome.engine_ms,
        outcome.ocr_ms,
        render_ms + outcome.total_ms,
        outcome.text.replace('\n', " ")
    );
    print_memory("after-capture-ocr");

    std::thread::sleep(Duration::from_secs(idle_seconds));
    print_memory("after-idle");
    Ok(())
}

fn run_path_benchmark() -> AnyResult<()> {
    const PATH_TEXT: &str = r"C:\Users\VM764NY\Downloads\instocr\experiments\rust";

    println!("InstOCR Rust path OCR benchmark");
    println!("expected=\"{PATH_TEXT}\"");
    print_memory("startup");

    let font = load_font();
    let mut ocr = OcrController::new()?;
    for case in [
        PathBenchmarkCase {
            name: "light",
            background: [255, 255, 255, 255],
            foreground: [0, 0, 0, 255],
        },
        PathBenchmarkCase {
            name: "dark",
            background: [12, 12, 12, 255],
            foreground: [235, 235, 235, 255],
        },
    ] {
        let bgra =
            render_case_with_colors(&font, 960, 180, PATH_TEXT, case.background, case.foreground);
        let start = Instant::now();
        let outcome = ocr.recognize_bgra(960, 180, &bgra)?;
        println!(
            "path-benchmark {}: total={:.2}ms convert={:.2}ms engine={:.2}ms ocr={:.2}ms text=\"{}\"",
            case.name,
            elapsed_ms(start),
            outcome.convert_ms,
            outcome.engine_ms,
            outcome.ocr_ms,
            outcome.text.replace('\n', " ")
        );
    }

    print_memory("after-path-benchmark");
    Ok(())
}

fn run_capture_rect(args: &[String]) -> AnyResult<()> {
    if args.len() != 4 {
        return Err("--capture-rect requires X Y W H".into());
    }

    let x = args[0].parse::<i32>()?;
    let y = args[1].parse::<i32>()?;
    let width = args[2].parse::<i32>()?;
    let height = args[3].parse::<i32>()?;
    if width <= 0 || height <= 0 {
        return Err("capture rectangle width and height must be positive".into());
    }

    print_memory("startup");
    let mut ocr = OcrController::new()?;
    print_languages(&ocr);

    let capture_start = Instant::now();
    println!("capture-rect phase: capturing screen");
    let screen = capture_virtual_screen()?;
    println!(
        "capture-rect phase: captured virtual screen {}x{} at {},{}",
        screen.width, screen.height, screen.x, screen.y
    );
    let rect = RectI::new(
        x - screen.x,
        y - screen.y,
        x - screen.x + width,
        y - screen.y + height,
    );
    println!(
        "capture-rect phase: cropping rect {} {} {} {}",
        rect.left,
        rect.top,
        rect.width(),
        rect.height()
    );
    let image = crop_capture(&screen, rect)?;
    let capture_ms = elapsed_ms(capture_start);
    println!(
        "capture-rect phase: recognizing {}x{}",
        image.width, image.height
    );
    let outcome = ocr.recognize_bgra(image.width, image.height, &image.bgra)?;
    println!("capture-rect phase: copying clipboard");
    let clip_start = Instant::now();
    let clipboard_error = copy_text_to_clipboard(null_hwnd(), &outcome.text)
        .err()
        .map(|error| error.to_string());
    let clipboard_ms = elapsed_ms(clip_start);
    println!(
        "capture-rect-ocr: capture={:.2}ms convert={:.2}ms engine={:.2}ms ocr={:.2}ms clipboard={:.2}ms total={:.2}ms clipboard_error=\"{}\" text=\"{}\"",
        capture_ms,
        outcome.convert_ms,
        outcome.engine_ms,
        outcome.ocr_ms,
        clipboard_ms,
        capture_ms + outcome.total_ms + clipboard_ms,
        clipboard_error.unwrap_or_default(),
        outcome.text.replace('\n', " ")
    );
    print_memory("after-capture-ocr");
    Ok(())
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
    language: windows::Globalization::Language,
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
    let dst_stride = rect.width() as usize * 4;
    for row in 0..rect.height() as usize {
        let src_offset =
            ((rect.top as usize + row) * capture.width as usize + rect.left as usize) * 4;
        let dst_offset = row * dst_stride;
        bgra[dst_offset..dst_offset + dst_stride]
            .copy_from_slice(&capture.bgra[src_offset..src_offset + dst_stride]);
    }

    Ok(SelectedImage {
        width: rect.width(),
        height: rect.height(),
        bgra,
    })
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

fn load_font() -> Font {
    for candidate in [
        r"C:\Windows\Fonts\segoeui.ttf",
        r"C:\Windows\Fonts\arial.ttf",
    ] {
        if Path::new(candidate).exists() {
            let bytes = fs::read(candidate).expect("font file should be readable");
            return Font::from_bytes(bytes, FontSettings::default()).expect("font should parse");
        }
    }
    panic!("Unable to find a Windows UI font.");
}

fn render_case(font: &Font, width: i32, height: i32, text: &str) -> Vec<u8> {
    render_case_with_colors(
        font,
        width,
        height,
        text,
        [255, 255, 255, 255],
        [0, 0, 0, 255],
    )
}

fn render_case_with_colors(
    font: &Font,
    width: i32,
    height: i32,
    text: &str,
    background: [u8; 4],
    foreground: [u8; 4],
) -> Vec<u8> {
    let mut bgra = vec![0u8; width as usize * height as usize * 4];
    for pixel in bgra.chunks_exact_mut(4) {
        pixel.copy_from_slice(&background);
    }
    let font_size = (height / 12).max(18) as f32;
    let margin = 18;
    let line_height = (font_size * 1.35) as i32;
    let mut x = margin;
    let mut y = margin + font_size as i32;

    for word in text.split(' ') {
        let word_width = measure_word(font, font_size, word);
        if x + word_width > width - margin {
            x = margin;
            y += line_height;
        }

        for ch in word.chars().chain(std::iter::once(' ')) {
            let (metrics, bitmap) = font.rasterize(ch, font_size);
            let glyph_x = x + metrics.xmin;
            let glyph_y = y - metrics.height as i32 - metrics.ymin;
            for row in 0..metrics.height {
                for col in 0..metrics.width {
                    let dst_x = glyph_x + col as i32;
                    let dst_y = glyph_y + row as i32;
                    if dst_x < 0 || dst_x >= width || dst_y < 0 || dst_y >= height {
                        continue;
                    }
                    let coverage = bitmap[row * metrics.width + col] as u16;
                    let offset = (dst_y as usize * width as usize + dst_x as usize) * 4;
                    for channel in 0..3 {
                        let bg = background[channel] as u16;
                        let fg = foreground[channel] as u16;
                        let blended = (bg * (255 - coverage) + fg * coverage) / 255;
                        bgra[offset + channel] = blended as u8;
                    }
                    bgra[offset + 3] = 255;
                }
            }
            x += metrics.advance_width.ceil() as i32;
        }
    }
    bgra
}

struct PathBenchmarkCase {
    name: &'static str,
    background: [u8; 4],
    foreground: [u8; 4],
}

fn measure_word(font: &Font, font_size: f32, word: &str) -> i32 {
    word.chars()
        .chain(std::iter::once(' '))
        .map(|ch| font.metrics(ch, font_size).advance_width.ceil() as i32)
        .sum()
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

fn null_hwnd() -> HWND {
    HWND(null_mut())
}
