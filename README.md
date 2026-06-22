# InstOCR

InstOCR is a lightweight Windows tray tool for instant OCR screen capture. Press a hotkey, drag around text, and the recognized text is copied to your clipboard.

The Rust version is now the production version. In manual Task Manager profiling, the earlier C# WPF build was around **140 MiB after first use**, while this Rust resident app peaked around **5.5 MiB** on the user's machine: more than **10x lower observed memory** for the workflow that matters here.

## Install for normal users

1. Extract the folder.
2. Double-click:

   ```text
   Install InstOCR.cmd
   ```

   Or run:

   ```powershell
   powershell -ExecutionPolicy Bypass -File .\install.ps1
   ```

3. InstOCR starts immediately and is registered to run at Windows login.
4. Use:
   - `Ctrl+Alt+D` to capture text.
   - `Ctrl+Alt+W` to cycle OCR language.
   - Right-click the tray icon to capture, cycle language, toggle startup, or exit.

If Windows SmartScreen appears, choose **More info -> Run anyway**. This build is unsigned.

## Change hotkeys

Double-click:

```text
Configure Hotkeys.cmd
```

Or run:

```powershell
powershell -ExecutionPolicy Bypass -File .\configure-hotkeys.ps1
```

Supported hotkeys require `Ctrl` or `Alt`, with optional `Shift`, plus one key:

```text
Ctrl+Alt+D
Ctrl+Shift+P
Alt+F8
Ctrl+Alt+PrintScreen
```

The Windows key is intentionally rejected because many Win-key shortcuts are reserved by Windows. `F12` is rejected because it is reserved for debuggers.

Settings are stored at:

```text
%APPDATA%\InstOCR\rust-settings.txt
```

## Uninstall

Run:

```powershell
powershell -ExecutionPolicy Bypass -File .\uninstall.ps1
```

This removes the per-user startup entry and installed files under:

```text
%LOCALAPPDATA%\InstOCR
```

## OCR languages

InstOCR uses Windows built-in OCR (`Windows.Media.Ocr`). It does not bundle an OCR model and does not use the cloud.

- `Auto` uses Windows profile languages.
- French works best when a French Windows OCR language such as `fr-FR` is installed and selected.
- Arabic requires installing an Arabic Windows OCR language pack first.

Check installed OCR languages:

```powershell
.\dist\windows-x64\instocr-cli.exe --list-languages
```

## Rust commands

Resident tray app:

```powershell
.\dist\windows-x64\instocr.exe
```

CLI diagnostics:

```powershell
.\dist\windows-x64\instocr-cli.exe --list-languages
.\dist\windows-x64\instocr-cli.exe --measure --idle-seconds 2
.\dist\windows-x64\instocr-cli.exe --path-benchmark
.\dist\windows-x64\instocr-cli.exe --capture-rect X Y W H
```

`--capture-rect` uses physical pixels. On a 150% scaled display, coordinates from some tools may need to be multiplied by `1.5`. For normal use, prefer `Ctrl+Alt+D` drag-select.

## Notes

- The install is per-user. No admin rights are required.
- Startup uses the current user's `HKCU\Software\Microsoft\Windows\CurrentVersion\Run` key.
- The resident app is `instocr.exe`.
- The diagnostic CLI is `instocr-cli.exe`.
- Mixed-DPI multi-monitor selection is still approximate in this Rust build; single-monitor and same-DPI setups are the target.
