#!/usr/bin/env pwsh

# Ensure script runs from its own directory
Set-Location -Path $PSScriptRoot

Write-Host " Extracting server licenses (Rust)"

# Remove old license files
$serverLicense = "../THIRD_PARTY_LICENSES.md"
$adminLicense = "../www/THIRD_PARTY_LICENSES.md"

if (Test-Path -Path $serverLicense) {
    Remove-Item -Force $serverLicense
}
if (Test-Path -Path $adminLicense) {
    Remove-Item -Force $adminLicense
}

# Generate license report for Windows target
cargo about generate markdown.hbs `
    --manifest-path ../Cargo.toml `
    --target x86_64-pc-windows-msvc `
    -o temp-windows.md

# Generate license report for Linux target
cargo about generate markdown.hbs `
    --manifest-path ../Cargo.toml `
    --target x86_64-unknown-linux-gnu `
    -o temp-linux.md

# Merge both into a single file
"# Combined License Report (Windows + Linux)`n" | Out-File $serverLicense -Encoding utf8
Get-Content temp-windows.md | Add-Content $serverLicense
"`n---`n" | Add-Content $serverLicense
Get-Content temp-linux.md | Add-Content $serverLicense

# Clean up temp files
Remove-Item temp-windows.md, temp-linux.md

Write-Host " Extracting admin console licenses (Node.js)"
npx license-report --production --package=../www/package.json --output=markdown > $adminLicense

Write-Host " Done"