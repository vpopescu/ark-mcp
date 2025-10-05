#!/usr/bin/env pwsh

Write-host "Extracting server licenses"

if (Test-Path -Path ../THIRD_PARTY_LICENSES.md) {
    Remove-Item -Force  ../THIRD_PARTY_LICENSES.md
}

if (Test-Path -Path ../www/THIRD_PARTY_LICENSES.md) {
    Remove-Item -Force  ../www/THIRD_PARTY_LICENSES.md
}


cargo about generate markdown.hbs  --manifest-path ../Cargo.toml  -o ../THIRD_PARTY_LICENSES.md
Write-host "Extracting admin console licenses"
npx license-report --production --package=../www/package.json --output=markdown > ../www/THIRD_PARTY_LICENSES.md
Write-Host "Done"

