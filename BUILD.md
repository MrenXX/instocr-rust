# Build InstOCR

## Requirements

- Windows
- Rust toolchain (`cargo`, `rustc`)

This machine has Rust at:

```text
C:\Users\VM764NY\.cargo\bin
```

## Build

```powershell
cargo build --release
```

Outputs:

```text
target\release\instocr.exe
target\release\instocr-cli.exe
```

The project uses `.cargo\config.toml` with `target-feature=+crt-static` so the prebuilt binaries do not require `vcruntime140.dll`.

## Refresh prebuilt binaries

```powershell
cargo build --release
New-Item -ItemType Directory -Force -Path .\dist\windows-x64
Copy-Item .\target\release\instocr.exe .\dist\windows-x64\instocr.exe -Force
Copy-Item .\target\release\instocr-cli.exe .\dist\windows-x64\instocr-cli.exe -Force
```

## Validate

```powershell
.\dist\windows-x64\instocr-cli.exe --list-languages
.\dist\windows-x64\instocr-cli.exe --measure --idle-seconds 2
.\dist\windows-x64\instocr-cli.exe --path-benchmark
```
