<#
.SYNOPSIS
Builds the corvus-regex-ecma262 C library and lays it out as a release package.

.DESCRIPTION
Builds the shared and static libraries (cargo, release profile) for a Rust target, and writes
<OutDir>/corvus-regex-ecma262-<version>-<target>/ with:

  include/                       corvus_regex_ecma262.h, which documents every function
  lib/ (and bin/ on Windows)     the shared library (with its import library on Windows) and the static library; for
                                 MSVC also corvus_regex_ecma262_mt.lib, the static library for the static C runtime (/MT)
  lib/cmake/corvus_regex_ecma262/  the CMake package: corvus_regex_ecma262::corvus_regex_ecma262 (shared) and
                                 corvus_regex_ecma262::corvus_regex_ecma262_static, with a version file
  lib/pkgconfig/                 corvus-regex-ecma262.pc
  LICENSE

The static library needs the system libraries the Rust standard library uses; they are asked of the compiler
(--print native-static-libs) and written into the CMake package and the pkg-config file.

.PARAMETER Target
The Rust target triple (default: the host's).

.PARAMETER Zig
Build with cargo-zigbuild (zig as the linker), to cross-build Linux targets such as the musl ones from a glibc host.

.PARAMETER Glibc
With -Zig, the oldest glibc the shared library may need (such as 2.17, the manylinux2014 baseline): it links against
that version's symbols, so it loads on any distribution with that glibc or newer.

.PARAMETER OutDir
Where to write the package (default: dist, next to this script). The version is this crate's (Cargo.toml), which is
the engine's.

.EXAMPLE
pwsh package.ps1
pwsh package.ps1 -Target aarch64-unknown-linux-gnu -OutDir /tmp/packages
#>
[CmdletBinding()]
param(
    [string] $Target,
    [string] $OutDir = (Join-Path $PSScriptRoot "dist"),
    [switch] $Zig,
    [string] $Glibc
)

$ErrorActionPreference = "Stop"
Set-StrictMode -Version Latest

function Invoke-Native([string] $Command, [string[]] $Arguments) {
    $output = & $Command @Arguments 2>&1
    if ($LASTEXITCODE -ne 0) {
        $output | Write-Host
        throw "$Command $($Arguments -join ' ') failed (exit $LASTEXITCODE)"
    }
    return $output
}

Push-Location $PSScriptRoot
try {
    if (-not $Target) {
        $Target = ((Invoke-Native "rustc" @("-vV")) | Where-Object { $_ -like "host: *" }) -replace "^host: ", ""
    }
    $version = ((Get-Content Cargo.toml) | Where-Object { $_ -match '^version = "(.+)"$' } | Select-Object -First 1) -replace '^version = "(.+)"$', '$1'
    $targetIsWindows = $Target -like "*-windows-*"
    $targetIsMac = $Target -like "*-apple-*"
    $targetIsMsvc = $Target -like "*-msvc"

    # A musl shared library links the C runtime dynamically (the musl targets link it statically by default, which a
    # shared library cannot); the static library is unaffected.
    if ($Target -like "*-musl*") {
        $env:RUSTFLAGS = "$env:RUSTFLAGS -C target-feature=-crt-static".Trim()
    }
    $build = if ($Zig) { "zigbuild" } else { "build" }
    $targets = @($Target)
    foreach ($t in $targets) {
        Write-Host "Building corvus_regex_ecma262 $version for $t"
        # cargo-zigbuild takes the glibc version as a suffix on the target (the output directory is the target's).
        $buildTarget = if ($Glibc) { "$t.$Glibc" } else { $t }
        Invoke-Native "cargo" @($build, "-p", "corvus-regex-ecma262-capi", "--lib", "--release", "--locked", "--target", $buildTarget, "-j", "4") | Out-Null
    }
    # For MSVC, a second static library linked against the static C runtime (/MT), built apart so that the first is
    # untouched: the default one matches only the dynamic runtime (/MD).
    $staticMt = ""
    if ($targetIsMsvc) {
        $saved = $env:RUSTFLAGS
        $env:RUSTFLAGS = "$saved -C target-feature=+crt-static".Trim()
        $env:CARGO_TARGET_DIR = Join-Path ".." "target" "crt-static"
        try {
            Invoke-Native "cargo" @("rustc", "-p", "corvus-regex-ecma262-capi", "--lib", "--crate-type", "staticlib", "--release", "--locked", "--target", $Target, "-j", "4") | Out-Null
        } finally {
            $env:RUSTFLAGS = $saved
            Remove-Item Env:CARGO_TARGET_DIR
        }
        $staticMt = "lib/corvus_regex_ecma262_mt.lib"
    }
    # The system libraries a program linking the static library needs (building a static library links nothing, so
    # this needs no cross linker).
    $cargoArgs = @("-p", "corvus-regex-ecma262-capi", "--release", "--locked", "--target", $targets[0], "-j", "4")
    $printed = Invoke-Native "cargo" (@("rustc", "--lib", "--crate-type", "staticlib") + $cargoArgs + @("--", "--print", "native-static-libs"))
    $line = $printed | Where-Object { "$_" -match "native-static-libs: " } | Select-Object -Last 1
    if (-not $line) { throw "rustc did not print the native static libraries" }
    # An array (@(...)): a single library would otherwise be a string, indexed by character below.
    $nativeLibs = @(("$line" -replace "^.*native-static-libs: ", "").Trim() -split "\s+" | Where-Object { $_ })

    # CMake wants library names (and frameworks as "-framework X"); pkg-config wants linker flags.
    $cmakeLibs = [System.Collections.Generic.List[string]]::new()
    $pcFlags = [System.Collections.Generic.List[string]]::new()
    for ($i = 0; $i -lt $nativeLibs.Count; $i++) {
        $lib = $nativeLibs[$i]
        if ($lib -eq "-framework") {
            $cmakeLibs.Add("-framework $($nativeLibs[$i + 1])")
            $pcFlags.Add("-framework $($nativeLibs[$i + 1])")
            $i++
        } elseif ($lib -like "-l*") {
            $cmakeLibs.Add($lib.Substring(2))
            $pcFlags.Add($lib)
        } elseif ($lib -like "/defaultlib:*") {
            # The C runtime: CMake's and MSVC's default (the dynamic one) is the one Rust links against.
            continue
        } elseif ($lib -like "*.lib") {
            $cmakeLibs.Add($lib)
            $pcFlags.Add($lib)
        } else {
            $cmakeLibs.Add($lib)
            $pcFlags.Add($lib)
        }
    }

    # The workspace's target directory, one level up.
    $built = Join-Path ".." "target" $Target "release"
    $name = "corvus-regex-ecma262-$version-$Target"
    $root = Join-Path $OutDir $name
    if (Test-Path $root) { Remove-Item $root -Recurse -Force }
    $cmakeDir = Join-Path $root "lib" "cmake" "corvus_regex_ecma262"
    $pcDir = Join-Path $root "lib" "pkgconfig"
    foreach ($d in @("include", "lib")) { New-Item -ItemType Directory -Force (Join-Path $root $d) | Out-Null }
    New-Item -ItemType Directory -Force $cmakeDir, $pcDir | Out-Null

    Copy-Item include/corvus_regex_ecma262.h (Join-Path $root "include")
    Copy-Item LICENSE $root

    $importLibrary = ""
    if ($targetIsWindows) {
        New-Item -ItemType Directory -Force (Join-Path $root "bin") | Out-Null
        Copy-Item (Join-Path $built "corvus_regex_ecma262.dll") (Join-Path $root "bin")
        if ($targetIsMsvc) {
            Copy-Item (Join-Path $built "corvus_regex_ecma262.dll.lib") (Join-Path $root "lib")
            Copy-Item (Join-Path $built "corvus_regex_ecma262.lib") (Join-Path $root "lib")
            Copy-Item (Join-Path ".." "target" "crt-static" $Target "release" "corvus_regex_ecma262.lib") (Join-Path $root $staticMt)
            $importLibrary = "lib/corvus_regex_ecma262.dll.lib"
            $static = "lib/corvus_regex_ecma262.lib"
        } else {
            Copy-Item (Join-Path $built "libcorvus_regex_ecma262.dll.a") (Join-Path $root "lib")
            Copy-Item (Join-Path $built "libcorvus_regex_ecma262.a") (Join-Path $root "lib")
            $importLibrary = "lib/libcorvus_regex_ecma262.dll.a"
            $static = "lib/libcorvus_regex_ecma262.a"
        }
        $shared = "bin/corvus_regex_ecma262.dll"
    } else {
        $sharedName = if ($targetIsMac) { "libcorvus_regex_ecma262.dylib" } else { "libcorvus_regex_ecma262.so" }
        Copy-Item (Join-Path $built $sharedName) (Join-Path $root "lib")
        Copy-Item (Join-Path $built "libcorvus_regex_ecma262.a") (Join-Path $root "lib")
        $shared = "lib/$sharedName"
        $static = "lib/libcorvus_regex_ecma262.a"
    }

    $values = @{
        "@PLATFORM@"              = $Target
        "@VERSION@"               = $version
        "@SHARED_LOCATION@"       = $shared
        "@IMPORT_LIBRARY@"        = $importLibrary
        "@STATIC_LOCATION@"       = $static
        "@STATIC_MT_LOCATION@"    = $staticMt
        "@STATIC_LINK_LIBRARIES@" = ($cmakeLibs -join ";")
        "@STATIC_LINK_FLAGS@"     = ($pcFlags -join " ")
    }
    foreach ($t in @(
            @("cmake/corvus_regex_ecma262Config.cmake.in", (Join-Path $cmakeDir "corvus_regex_ecma262Config.cmake")),
            @("cmake/corvus_regex_ecma262ConfigVersion.cmake.in", (Join-Path $cmakeDir "corvus_regex_ecma262ConfigVersion.cmake")),
            @("cmake/corvus-regex-ecma262.pc.in", (Join-Path $pcDir "corvus-regex-ecma262.pc")))) {
        $text = Get-Content $t[0] -Raw
        foreach ($k in $values.Keys) { $text = $text.Replace($k, $values[$k]) }
        Set-Content -Path $t[1] -Value $text -NoNewline
    }

    Write-Host "Packaged $root"
    Write-Output $root
} finally {
    Pop-Location
}
