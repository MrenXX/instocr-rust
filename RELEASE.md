# Release and Git bundle

This repository is intended to be shared without pushing from this machine's GitHub account.

## Create a local commit

```powershell
git init
git config user.name "InstOCR Builder"
git config user.email "instocr@example.invalid"
git add -A
git commit -m "Initial Rust InstOCR release"
```

## Create bundle

```powershell
git bundle create dist\instocr-rust.bundle --all
git bundle verify dist\instocr-rust.bundle
git bundle list-heads dist\instocr-rust.bundle
```

## Verify bundle in a temp clone

```powershell
$tmp = Join-Path $env:TEMP "instocr-bundle-check"
Remove-Item -Recurse -Force $tmp -ErrorAction SilentlyContinue
git clone dist\instocr-rust.bundle $tmp
Set-Location $tmp
cargo build --release
```

## Optional zip for non-Git users

```powershell
Compress-Archive -Path .\dist\windows-x64\*, .\install.ps1, '.\Install InstOCR.cmd', .\configure-hotkeys.ps1, '.\Configure Hotkeys.cmd', .\uninstall.ps1, .\README.md -DestinationPath .\dist\instocr-rust-windows-x64.zip -Force
```

## Notes

- Binaries are unsigned; SmartScreen may warn users.
- Use `Unblock-File` if Windows marks downloaded scripts or exes as blocked.
- Do not commit generated `.bundle` files.
