<#
.SYNOPSIS
  Build, sign and package MailCraft for Windows.

.DESCRIPTION
  Produces, in $env:DIST (default: dist/release):
    mailcraft-<version>-windows-<arch>.msi            per-machine installer (WiX v5)
    mailcraft-<version>-windows-<arch>-portable.zip   app, CLI and (x64) native content runtime

  The binaries link the C runtime statically (+crt-static), so neither the MSI nor the portable
  zip needs the Visual C++ redistributable. Signing is delegated to sign.ps1 (skipped with a
  warning when no signing secrets are set).

  Needs: Rust (MSVC toolchain + the target), the Windows SDK (rc.exe, signtool.exe),
  and WiX v5 with its UI and Util extensions (installer dialogs, "Launch MailCraft" on finish):
    dotnet tool install --global wix --version 5.0.2
    wix extension add -g WixToolset.UI.wixext/5.0.2
    wix extension add -g WixToolset.Util.wixext/5.0.2

.EXAMPLE
  pwsh packaging/windows/package.ps1 -Arch x64
  pwsh packaging/windows/package.ps1 -Arch x86 -SkipBuild
  pwsh packaging/windows/package.ps1 -Arch x64 -BuildOnly   # compile only (CI signs in another job: -SkipBuild -NoRun)
  pwsh packaging/windows/package.ps1 -Arch arm64     # cross-compiled; needs the MSVC ARM64 build tools
#>
param(
  [ValidateSet('x64', 'x86', 'arm64')] [string] $Arch = 'x64',
  [switch] $SkipBuild,
  [switch] $BuildOnly,
  # Exact qualified runtime input; mandatory for x64 packaging, not for -BuildOnly.
  [string] $ContentRuntime = $env:MAILCRAFT_CONTENT_RUNTIME,
  # Fresh disposable package scratch. Retire through transactional-cleanup after last use.
  [string] $PackageRoot = '',
  # Don't run the built CLI at the end (the job that signs never runs what was built).
  [switch] $NoRun
)
$ErrorActionPreference = 'Stop'
if (Test-Path -LiteralPath (Join-Path $PSScriptRoot '../../plan/COMPILATION-SUSPENDED.md')) {
  throw 'The user suspended compilation and candidate/package validation until whole-project source readiness'
}
$Root = (Resolve-Path (Join-Path $PSScriptRoot '..\..')).Path

function Invoke-Native([string] $What, [scriptblock] $Block) {
  Write-Output "==> $What"
  & $Block
  if ($LASTEXITCODE -ne 0) { throw "$What failed with exit code $LASTEXITCODE" }
}

# The version lives in one place: [workspace.package] version in the root Cargo.toml.
$Version = $env:MAILCRAFT_VERSION
if (-not $Version) {
  $inPkg = $false
  foreach ($line in Get-Content (Join-Path $Root 'Cargo.toml')) {
    if ($line -match '^\s*\[') { $inPkg = ($line.Trim() -eq '[workspace.package]'); continue }
    if ($inPkg -and $line -match '^\s*version\s*=\s*"([^"]+)"') { $Version = $Matches[1]; break }
  }
}
if (-not $Version) { throw 'could not read [workspace.package] version from Cargo.toml' }
# MSI ProductVersion is numeric (major.minor.build); pre-release tags are dropped there.
$MsiVersion = ($Version -split '-')[0]

$Target = switch ($Arch) { 'x64' { 'x86_64-pc-windows-msvc' } 'x86' { 'i686-pc-windows-msvc' } 'arm64' { 'aarch64-pc-windows-msvc' } }
$Dist = if ($env:DIST) { $env:DIST } else { Join-Path $Root 'dist\release' }
$TargetDir = if ($env:CARGO_TARGET_DIR) { $env:CARGO_TARGET_DIR } else { Join-Path $Root 'target' }
New-Item -ItemType Directory -Force -Path $Dist | Out-Null
if (-not $PackageRoot) {
  # Native/package small-file I/O stays on D: on the maintained Windows host.
  $scratchBase = if ($env:GITHUB_ACTIONS -eq 'true') { $env:RUNNER_TEMP } else { 'D:\Temp' }
  $PackageRoot = Join-Path $scratchBase ("mailcraft-package-$Version-$Arch-" + [guid]::NewGuid().ToString('N'))
}
$PackageRoot = [IO.Path]::GetFullPath($PackageRoot)
if (Test-Path -LiteralPath $PackageRoot) { throw 'Choose a new exact package scratch root; existing output is preserved' }
New-Item -ItemType Directory -Path $PackageRoot | Out-Null
$previousTemp = $env:TEMP
$previousTmp = $env:TMP
$env:TEMP = $PackageRoot
$env:TMP = $PackageRoot
try {
[ordered]@{version=$Version;arch=$Arch;packageScratch=$PackageRoot;contentRuntime=$ContentRuntime;cleanup='transactional-cleanup after all consumers exit';storageReason='native/package small-file I/O'} |
  ConvertTo-Json | Set-Content -LiteralPath (Join-Path $PackageRoot 'package-inputs.json') -Encoding utf8

if (-not $env:MAILCRAFT_BUILD_SHA) { $env:MAILCRAFT_BUILD_SHA = (git -C $Root rev-parse HEAD 2>$null) }
if (-not $env:MAILCRAFT_BUILD_DATE) { $env:MAILCRAFT_BUILD_DATE = (Get-Date).ToUniversalTime().ToString('yyyy-MM-dd') }

Write-Output "MailCraft $Version for Windows $Arch ($Target)"

if (-not $SkipBuild) {
  # Static CRT: no VC++ redistributable needed. Scoped to the target so host build scripts and
  # proc-macros are unaffected.
  $flagVar = 'CARGO_TARGET_' + ($Target.ToUpper() -replace '-', '_') + '_RUSTFLAGS'
  [Environment]::SetEnvironmentVariable($flagVar, '-C target-feature=+crt-static')
  # Fail the build (rather than warn) if the icon/VERSIONINFO can't be embedded.
  $env:MAILCRAFT_REQUIRE_WINRES = '1'
  Invoke-Native "cargo build ($Target)" { cargo build --release --locked -p mailcraft -p mailcraft-cli --target $Target }
}

$Bin = Join-Path $TargetDir "$Target\release"

# CI compiles in a job of its own (-BuildOnly), which has no signing secret, and hands the binaries
# by digest to a job that signs and packages them (-SkipBuild -NoRun) without running cargo or them.
if ($BuildOnly) { Write-Output 'built; signing and packaging come next'; exit 0 }

# Check both binaries' PE headers before packaging. Machine (COFF header) must match -Arch, so an
# x64 build can never ship labelled arm64. Subsystem (optional header): 2 = Windows GUI, 3 = console.
# The app must be GUI (no console window opens with it); the CLI must stay console so its output
# reaches the terminal.
function Get-PeHeader([string] $Path) {
  $bytes = [System.IO.File]::ReadAllBytes($Path)
  $pe = [BitConverter]::ToInt32($bytes, 0x3C)
  return @{ Machine = [BitConverter]::ToUInt16($bytes, $pe + 4); Subsystem = [BitConverter]::ToUInt16($bytes, $pe + 0x5C) }
}
$Machine = switch ($Arch) { 'x64' { 0x8664 } 'x86' { 0x14C } 'arm64' { 0xAA64 } }
foreach ($check in @(@('mailcraft.exe', 2), @('mailcraft-cli.exe', 3))) {
  $h = Get-PeHeader (Join-Path $Bin $check[0])
  if ($h.Machine -ne $Machine) { throw "$($check[0]) is for machine 0x$('{0:X}' -f $h.Machine), expected 0x$('{0:X}' -f $Machine) ($Arch)" }
  if ($h.Subsystem -ne $check[1]) { throw "$($check[0]) has PE subsystem $($h.Subsystem), expected $($check[1])" }
  Write-Output "ok $($check[0]): $Arch, PE subsystem $($h.Subsystem)"
}
$Stage = Join-Path $PackageRoot 'stage'
New-Item -ItemType Directory -Path $Stage | Out-Null
Copy-Item (Join-Path $Bin 'mailcraft.exe'), (Join-Path $Bin 'mailcraft-cli.exe') $Stage
Copy-Item (Join-Path $Root 'NOTICE'), (Join-Path $Root 'ATTRIBUTION.md') $Stage
$RuntimeFragment = Join-Path $PackageRoot 'content-runtime.wxs'
$runtimeArgs = @('-B', (Join-Path $PSScriptRoot 'content-runtime.py'), '--arch', $Arch, '--stage', $Stage, '--fragment', $RuntimeFragment)
if ($ContentRuntime) { $runtimeArgs += @('--source', $ContentRuntime) }
Invoke-Native 'bind qualified content runtime' { & python @runtimeArgs }

& (Join-Path $PSScriptRoot 'sign.ps1') (Join-Path $Stage 'mailcraft.exe') (Join-Path $Stage 'mailcraft-cli.exe')

# ---- MSI ---------------------------------------------------------------------------------------
# The installer's welcome page shows the license: build its RTF from LICENSE-MIT and LICENSE-APACHE
# so it never drifts from them. Hard-wrapped lines are rejoined into paragraphs so the text wraps
# to the dialog's width.
function ConvertTo-RtfText([string] $Text) {
  $sb = [System.Text.StringBuilder]::new()
  foreach ($ch in $Text.ToCharArray()) {
    $c = [int] $ch
    if ($c -eq 0x5C -or $c -eq 0x7B -or $c -eq 0x7D) { [void] $sb.Append('\').Append($ch) }
    elseif ($c -gt 127) { [void] $sb.Append("\u$(if ($c -gt 32767) { $c - 65536 } else { $c })?") }
    else { [void] $sb.Append($ch) }
  }
  return $sb.ToString()
}
$paragraphs = [System.Collections.Generic.List[string]]::new()
$paragraphs.Add('\b MailCraft\b0')
$paragraphs.Add((ConvertTo-RtfText 'MailCraft is dual-licensed under the MIT License or the Apache License, Version 2.0, at your option. Both license texts follow. Bundled fonts and other third-party material keep their own open licenses, listed in NOTICE, ATTRIBUTION.md and craft-content/licences at https://github.com/Darkaxt/mailcraft.'))
foreach ($file in 'LICENSE-MIT', 'LICENSE-APACHE') {
  $title = $true  # each file opens with its license's name: show it bold
  foreach ($para in ((Get-Content -Raw (Join-Path $Root $file)) -split '\r?\n\s*\r?\n')) {
    $joined = (($para -split '\r?\n') | ForEach-Object { $_.Trim() } | Where-Object { $_ }) -join ' '
    if (-not $joined) { continue }
    $paragraphs.Add($(if ($title) { "\b $(ConvertTo-RtfText $joined)\b0" } else { ConvertTo-RtfText $joined }))
    $title = $false
  }
}
$LicenseRtf = Join-Path $Stage 'license.rtf'
$rtf = '{\rtf1\ansi\ansicpg1252\deff0{\fonttbl{\f0\fswiss\fcharset0 Tahoma;}}\viewkind4\uc1\pard\sa120\f0\fs16 ' +
  ($paragraphs -join "\par`r`n") + "\par`r`n}`r`n"
[System.IO.File]::WriteAllText($LicenseRtf, $rtf, [System.Text.Encoding]::ASCII)

$Msi = Join-Path $Dist "mailcraft-$Version-windows-$Arch.msi"
if (Test-Path -LiteralPath $Msi) { throw 'Preserve the existing MSI deliverable; select a fresh DIST' }
$BuiltMsi = Join-Path $PackageRoot (Split-Path -Leaf $Msi)
Invoke-Native 'wix build' {
  wix build (Join-Path $PSScriptRoot 'mailcraft.wxs') $RuntimeFragment -arch $Arch `
    -ext WixToolset.UI.wixext -ext WixToolset.Util.wixext `
    -d "Version=$MsiVersion" -d "BinDir=$Stage" -d "IconPath=$(Join-Path $Root 'assets\app-icon\mailcraft.ico')" `
    -d "LicenseRtf=$LicenseRtf" `
    -o $BuiltMsi
}
# WiX's debug sidecar stays in exact package scratch, outside release deliverables.
Copy-Item -LiteralPath $BuiltMsi -Destination $Msi
& (Join-Path $PSScriptRoot 'sign.ps1') $Msi

# ---- portable zip ------------------------------------------------------------------------------
$Portable = Join-Path $PackageRoot "mailcraft-$Version-windows-$Arch-portable"
New-Item -ItemType Directory -Path $Portable | Out-Null
Copy-Item (Join-Path $Stage '*.exe') $Portable
if ($Arch -eq 'x64') { Copy-Item -LiteralPath (Join-Path $Stage 'craft-content') -Destination $Portable -Recurse }
foreach ($f in 'README.md', 'LICENSE', 'LICENSE-MIT', 'LICENSE-APACHE', 'NOTICE', 'ATTRIBUTION.md') {
  $p = Join-Path $Root $f
  if (Test-Path $p) { Copy-Item $p $Portable }
}
# Builds made with craft-fonts (CRAFT_FONTS_DIR) embed its fonts: ship their licences.
if ($env:CRAFT_FONTS_DIR) {
  foreach ($ofl in Get-ChildItem -Path (Join-Path $env:CRAFT_FONTS_DIR 'fonts\*\OFL.txt') -ErrorAction SilentlyContinue) {
    Copy-Item $ofl.FullName (Join-Path $Portable "OFL-$($ofl.Directory.Name).txt")
  }
}
$Zip = Join-Path $Dist "mailcraft-$Version-windows-$Arch-portable.zip"
if (Test-Path -LiteralPath $Zip) { throw 'Preserve the existing portable deliverable; select a fresh DIST' }
Compress-Archive -Path $Portable -DestinationPath $Zip

# Smoke-test the CLI when this machine can run it. An ARM64 build made on an x64 runner can't run
# here; .github/workflows/windows-arm64.yml installs and runs it on ARM64 instead.
$HostArch = [System.Runtime.InteropServices.RuntimeInformation]::OSArchitecture.ToString().ToLowerInvariant()
if ($NoRun) {
  Write-Output 'skipping mailcraft-cli --version (-NoRun): a job without the signing secrets runs it'
} elseif ($Arch -ne 'arm64' -or $HostArch -eq 'arm64') {
  Invoke-Native 'mailcraft-cli --version' { & (Join-Path $Stage 'mailcraft-cli.exe') --version }
} else {
  Write-Output "skipping mailcraft-cli --version: an $Arch build doesn't run on this $HostArch machine"
}
Get-Item $Msi, $Zip | Format-Table Name, Length
Write-Output "Package scratch retained for transactional cleanup after verification: $PackageRoot"
} finally {
  $env:TEMP = $previousTemp
  $env:TMP = $previousTmp
}
