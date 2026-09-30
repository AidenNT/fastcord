#Requires -Version 5.1
<#
.SYNOPSIS
    Compila ecord con FFmpeg 7.0 + SDL2 en Windows.

.DESCRIPTION
    - Detecta Visual Studio (17 o 18) vía vswhere e importa vcvars64.bat
    - Detecta FFmpeg 7.0: prefiere C:\ffmpeg7 (manual), si no usa vcpkg
    - Instala FFmpeg 7.0 en vcpkg vía manifest mode si no existe
    - Detecta/instala LLVM para libclang (bindgen)
    - Detecta/instala NASM (requerido por BoringSSL/btls-sys)
    - Configura INCLUDE, LIBCLANG_PATH, BINDGEN_EXTRA_CLANG_ARGS, ASM_NASM
    - Compila el proyecto Rust
    - Copia las DLLs necesarias junto al .exe
    - Opcionalmente empaqueta todo en un ZIP portable

.PARAMETER Profile
    "debug" o "release". Por defecto "release".

.PARAMETER SkipVcpkgInstall
    Si se especifica, no instala FFmpeg/SDL2 vía vcpkg.

.PARAMETER Package
    Si se especifica, crea un ZIP portable con el exe + DLLs.

.EXAMPLE
    .\build.ps1
    .\build.ps1 -Profile debug
    .\build.ps1 -Package
    .\build.ps1 -SkipVcpkgInstall
#>

[CmdletBinding()]
param(
    [ValidateSet("debug", "release")]
    [string]$Profile = "release",

    [switch]$SkipVcpkgInstall,

    [switch]$Package
)

# ============================================================
# CONFIGURACIÓN
# ============================================================
$VcpkgRoot        = "C:\vcpkg"
$Triplet          = "x64-windows"
$ProjectRoot      = $PSScriptRoot
$BinaryName       = "ecord"                       # nombre del .exe sin extensión
$LlvmPath         = "C:\Program Files\LLVM\bin"
$ManualFfmpeg     = "C:\ffmpeg7"                  # FFmpeg 7.x precompilado (opcional)
$CmakeGen         = "Visual Studio 17 2022"
$VcpkgBaseline    = "c66e3c2eea8a82d78e6700d69af691ee9dfd689e"  # baseline con FFmpeg 7.0
$NasmVersion      = "2.16.03"                     # versión de NASM a descargar si falta
$NasmDownloadUrl  = "https://www.nasm.us/pub/nasm/releasebuilds/$NasmVersion/win64/nasm-$NasmVersion-win64.zip"
# ============================================================

# Continue (no Stop) para que stderr de cargo/vcpkg no aborte el script
$ErrorActionPreference = "Continue"
if ($PSVersionTable.PSVersion.Major -ge 7) {
    $PSNativeCommandUseErrorActionPreference = $false
}

# ---------- Helpers ----------
function Write-Step($msg) {
    Write-Host ""
    Write-Host "==> $msg" -ForegroundColor Cyan
}
function Write-Ok($msg)   { Write-Host "    OK: $msg" -ForegroundColor Green }
function Write-Warn($msg) { Write-Host "    ! $msg" -ForegroundColor Yellow }
function Write-Err($msg)  { Write-Host "    X $msg" -ForegroundColor Red }

function Invoke-Native {
    param(
        [Parameter(Mandatory)][string]$Exe,
        [Parameter(ValueFromRemainingArguments)][string[]]$Args,
        [switch]$IgnoreExitCode
    )
    & $Exe @Args
    $code = $LASTEXITCODE
    if (-not $IgnoreExitCode -and $code -ne 0) {
        throw "'$Exe $($Args -join ' ')' falló con código $code"
    }
}

function Refresh-PathFromRegistry {
    $machine = [Environment]::GetEnvironmentVariable("Path", "Machine")
    $user    = [Environment]::GetEnvironmentVariable("Path", "User")
    $env:Path = (@($machine, $user) | Where-Object { $_ }) -join ";"
}

# ============================================================
# 1. Verificar requisitos básicos
# ============================================================
Write-Step "Verificando requisitos"

foreach ($cmd in @("git", "cargo", "rustc")) {
    $found = Get-Command $cmd -ErrorAction SilentlyContinue
    if (-not $found) { throw "Falta '$cmd' en el PATH. Instálalo primero." }
    Write-Ok "$cmd -> $($found.Source)"
}

# ============================================================
# 2. Importar entorno de Visual Studio (INCLUDE, LIB, PATH)
# ============================================================
Write-Step "Detectando Visual Studio"

$vswhere = "${env:ProgramFiles(x86)}\Microsoft Visual Studio\Installer\vswhere.exe"
if (-not (Test-Path $vswhere)) {
    throw "No se encontró vswhere.exe. ¿Visual Studio instalado?"
}

$vsPath = & $vswhere -latest -products * `
    -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 `
    -property installationPath

if (-not $vsPath) {
    throw "No se encontró Visual Studio con toolchain C++ (x86.x64). Instala el workload 'Desktop development with C++'."
}
Write-Ok "Visual Studio en: $vsPath"

$vcvars = Join-Path $vsPath "VC\Auxiliary\Build\vcvars64.bat"
if (-not (Test-Path $vcvars)) {
    throw "No se encontró vcvars64.bat en $vcvars"
}

Write-Host "    Importando entorno de VS desde vcvars64.bat..." -ForegroundColor Gray
cmd /c "`"$vcvars`" >nul 2>&1 && set" | ForEach-Object {
    if ($_ -match '^([^=]+)=(.*)$') {
        [Environment]::SetEnvironmentVariable($matches[1], $matches[2], "Process")
    }
}

$includeCount = (($env:INCLUDE -split ';') | Where-Object { $_ }).Count
Write-Ok "INCLUDE tiene $includeCount rutas (MSVC + Windows SDK)"

# Guardar rutas de MSVC y Windows SDK para BINDGEN_EXTRA_CLANG_ARGS
$msvcInc = ($env:INCLUDE -split ';' | Where-Object { $_ -match "VC\\Tools\\MSVC" } | Select-Object -First 1)
$sdkPaths = ($env:INCLUDE -split ';' | Where-Object { $_ -match "Windows Kits" })

if ($msvcInc) { Write-Ok "MSVC include: $msvcInc" }
if ($sdkPaths) { Write-Ok "Windows SDK: $(($sdkPaths | Select-Object -First 1))" }

# ============================================================
# 3. Elegir fuente de FFmpeg: manual (C:\ffmpeg7) o vcpkg
# ============================================================
Write-Step "Determinando fuente de FFmpeg"

$useManualFfmpeg = Test-Path "$ManualFfmpeg\include\libavcodec\avfft.h"

if ($useManualFfmpeg) {
    Write-Ok "FFmpeg 7.x manual detectado en $ManualFfmpeg"
    $installedDir = $null
} else {
    Write-Warn "No hay FFmpeg manual en $ManualFfmpeg. Usando vcpkg."
    $installedDir = "$VcpkgRoot\installed\$Triplet"
}

# ============================================================
# 4. Instalar / verificar vcpkg (solo si no usamos FFmpeg manual)
# ============================================================
if (-not $useManualFfmpeg) {
    Write-Step "Verificando vcpkg en $VcpkgRoot"

    if (-not (Test-Path "$VcpkgRoot\vcpkg.exe")) {
        Write-Warn "vcpkg no encontrado. Clonando..."
        if (Test-Path $VcpkgRoot) { Remove-Item $VcpkgRoot -Recurse -Force }
        Invoke-Native git clone https://github.com/microsoft/vcpkg.git $VcpkgRoot
        Push-Location $VcpkgRoot
        try {
            Invoke-Native "$VcpkgRoot\bootstrap-vcpkg.bat"
        } finally {
            Pop-Location
        }
        Write-Ok "vcpkg instalado en $VcpkgRoot"
    } else {
        Write-Ok "vcpkg ya instalado"
    }

    # Instalar FFmpeg y SDL2 (manifest mode con baseline de FFmpeg 7.0)
    if (-not $SkipVcpkgInstall) {
        Write-Step "Instalando FFmpeg 7.0 + SDL2 vía vcpkg (manifest mode)"

        Push-Location $ProjectRoot
        try {
            Invoke-Native "$VcpkgRoot\vcpkg.exe" install "--triplet=$Triplet" "--x-manifest-root=$ProjectRoot"
        } finally {
            Pop-Location
        }
        Write-Ok "Paquetes vcpkg instalados"
    } else {
        Write-Warn "SkipVcpkgInstall activado, saltando instalación"
    }
}

# ============================================================
# 5. Verificar / instalar LLVM (libclang para bindgen)
# ============================================================
Write-Step "Verificando libclang para bindgen"

$libclangFound = $false

if ($env:LIBCLANG_PATH -and (Test-Path (Join-Path $env:LIBCLANG_PATH "libclang.dll"))) {
    Write-Ok "LIBCLANG_PATH ya definido: $env:LIBCLANG_PATH"
    $libclangFound = $true
}

if (-not $libclangFound) {
    $candidates = @($LlvmPath, "C:\Program Files (x86)\LLVM\bin")
    Get-ChildItem "C:\Program Files*\Microsoft Visual Studio\1*\*\VC\Tools\Llvm\x64\bin" -ErrorAction SilentlyContinue |
        ForEach-Object { $candidates += $_.FullName }

    foreach ($c in $candidates) {
        if (Test-Path (Join-Path $c "libclang.dll")) {
            [Environment]::SetEnvironmentVariable("LIBCLANG_PATH", $c, "User")
            $env:LIBCLANG_PATH = $c
            Write-Ok "LIBCLANG_PATH configurado: $c"
            $libclangFound = $true
            break
        }
    }
}

if (-not $libclangFound) {
    Write-Warn "libclang no encontrado. Instalando LLVM con winget..."
    if (Get-Command winget -ErrorAction SilentlyContinue) {
        Invoke-Native winget install --id LLVM.LLVM --silent --accept-package-agreements --accept-source-agreements -IgnoreExitCode
        if (Test-Path (Join-Path $LlvmPath "libclang.dll")) {
            [Environment]::SetEnvironmentVariable("LIBCLANG_PATH", $LlvmPath, "User")
            $env:LIBCLANG_PATH = $LlvmPath
            Write-Ok "LIBCLANG_PATH configurado: $LlvmPath"
        } else {
            Write-Warn "LLVM instalado pero libclang.dll no aparece todavía. Reinicia la terminal."
        }
    } else {
        throw "winget no disponible. Instala LLVM manualmente: https://releases.llvm.org/"
    }
}

# ============================================================
# 6. Verificar / instalar NASM (requerido por BoringSSL/btls-sys)
# ============================================================
Write-Step "Verificando NASM para BoringSSL (btls-sys)"

$nasmFound = $false
$nasmExe   = $null

# 6.1 — ¿Ya está en PATH?
$nasmCmd = Get-Command nasm -ErrorAction SilentlyContinue
if ($nasmCmd) {
    $nasmExe = $nasmCmd.Source
    Write-Ok "nasm en PATH: $nasmExe"
    $nasmFound = $true
}

# 6.2 — ¿ASM_NASM ya definido?
if (-not $nasmFound -and $env:ASM_NASM -and (Test-Path $env:ASM_NASM)) {
    $nasmExe = $env:ASM_NASM
    Write-Ok "ASM_NASM ya definido: $nasmExe"
    $nasmFound = $true
}

# 6.3 — Rutas comunes
if (-not $nasmFound) {
    $nasmCandidates = @(
        "C:\Program Files\NASM\nasm.exe",
        "C:\Program Files (x86)\NASM\nasm.exe",
        "C:\nasm\nasm.exe",
        "$env:LOCALAPPDATA\NASM\nasm.exe"
    )
    foreach ($c in $nasmCandidates) {
        if (Test-Path $c) {
            $nasmExe = $c
            Write-Ok "NASM encontrado en: $c"
            $nasmFound = $true
            break
        }
    }
}

# 6.4 — Intentar instalar con winget
if (-not $nasmFound) {
    Write-Warn "NASM no encontrado. Intentando instalar con winget..."
    if (Get-Command winget -ErrorAction SilentlyContinue) {
        Invoke-Native winget install --id NASM.NASM --silent `
            --accept-package-agreements --accept-source-agreements -IgnoreExitCode
        Refresh-PathFromRegistry
        $nasmCmd = Get-Command nasm -ErrorAction SilentlyContinue
        if ($nasmCmd) {
            $nasmExe = $nasmCmd.Source
            $nasmFound = $true
            Write-Ok "NASM instalado vía winget: $nasmExe"
        }
    } else {
        Write-Warn "winget no disponible."
    }
}

# 6.5 — Intentar instalar con chocolatey
if (-not $nasmFound) {
    if (Get-Command choco -ErrorAction SilentlyContinue) {
        Write-Warn "Intentando instalar NASM con chocolatey..."
        Invoke-Native choco install nasm -y -IgnoreExitCode
        Refresh-PathFromRegistry
        $nasmCmd = Get-Command nasm -ErrorAction SilentlyContinue
        if ($nasmCmd) {
            $nasmExe = $nasmCmd.Source
            $nasmFound = $true
            Write-Ok "NASM instalado vía choco: $nasmExe"
        }
    }
}

# 6.6 — Descarga directa desde nasm.us
if (-not $nasmFound) {
    Write-Warn "Descargando NASM $NasmVersion desde nasm.us..."
    try {
        $nasmZip     = Join-Path $env:TEMP "nasm-$NasmVersion.zip"
        $tmpExtract  = Join-Path $env:TEMP "nasm_extract"
        if (Test-Path $tmpExtract) { Remove-Item $tmpExtract -Recurse -Force }

        Invoke-WebRequest -Uri $NasmDownloadUrl -OutFile $nasmZip -UseBasicParsing
        Expand-Archive -Path $nasmZip -DestinationPath $tmpExtract -Force

        $nasmFoundExe = Get-ChildItem $tmpExtract -Filter "nasm.exe" -Recurse -ErrorAction SilentlyContinue |
                        Select-Object -First 1

        if ($nasmFoundExe) {
            $nasmTargetDir = Join-Path ${env:ProgramFiles} "NASM"
            if (-not (Test-Path $nasmTargetDir)) {
                New-Item -ItemType Directory -Path $nasmTargetDir -Force | Out-Null
            }
            Copy-Item (Join-Path (Split-Path $nasmFoundExe.FullName -Parent) '*') $nasmTargetDir -Recurse -Force
            $nasmExe   = Join-Path $nasmTargetDir "nasm.exe"
            $nasmFound = (Test-Path $nasmExe)
            if ($nasmFound) {
                Write-Ok "NASM instalado en: $nasmTargetDir"
            }
        }
        Remove-Item $nasmZip -Force -ErrorAction SilentlyContinue
        Remove-Item $tmpExtract -Recurse -Force -ErrorAction SilentlyContinue
    } catch {
        Write-Warn "No se pudo descargar/instalar NASM automáticamente: $_"
    }
}

if (-not $nasmFound) {
    throw @"
NASM es requerido para compilar BoringSSL (btls-sys) y no pudo instalarse automáticamente.
Instálalo manualmente:
  1. Descarga de https://www.nasm.us/pub/nasm/releasebuilds/$NasmVersion/win64/nasm-$NasmVersion-win64.zip
  2. Extrae nasm.exe (por ej. en C:\nasm)
  3. Añade esa carpeta al PATH o define la variable de entorno ASM_NASM=C:\ruta\nasm.exe
  4. Reinicia la terminal y vuelve a ejecutar este script.
"@
}

# Persistir y exportar variables que busca CMake / cmake-rs
$nasmDir = Split-Path $nasmExe -Parent
[Environment]::SetEnvironmentVariable("ASM_NASM", $nasmExe, "User")
[Environment]::SetEnvironmentVariable("CMAKE_ASM_NASM_COMPILER", $nasmExe, "User")
$env:ASM_NASM                  = $nasmExe
$env:CMAKE_ASM_NASM_COMPILER   = $nasmExe

if ($env:Path -notlike "*$nasmDir*") {
    $env:Path += ";$nasmDir"
    $userPath = [Environment]::GetEnvironmentVariable("Path", "User")
    if ($userPath -notlike "*$nasmDir*") {
        [Environment]::SetEnvironmentVariable("Path", "$userPath;$nasmDir", "User")
    }
}
Write-Ok "ASM_NASM = $nasmExe"
Write-Ok "CMAKE_ASM_NASM_COMPILER = $nasmExe"

# ============================================================
# 7. Configurar variables de entorno para la compilación
# ============================================================
Write-Step "Configurando variables de entorno"

# --- FFMPEG_DIR e INCLUDE ---
if ($useManualFfmpeg) {
    $ffmpegInclude = "$ManualFfmpeg\include"
    $ffmpegBin     = "$ManualFfmpeg\bin"
    $env:FFMPEG_DIR = $ManualFfmpeg
    Write-Ok "FFMPEG_DIR = $ManualFfmpeg (manual)"
} else {
    $ffmpegInclude = "$installedDir\include"
    $ffmpegBin     = "$installedDir\bin"
    $env:FFMPEG_DIR = $installedDir
    [Environment]::SetEnvironmentVariable("FFMPEG_DIR", $installedDir, "User")
    Write-Ok "FFMPEG_DIR = $installedDir (vcpkg)"
}

# --- INCLUDE: poner FFmpeg AL FRENTE, luego el resto de VS ---
if ($env:INCLUDE -notlike "*$ffmpegInclude*") {
    $env:INCLUDE = "$ffmpegInclude;$env:INCLUDE"
}
Write-Ok "INCLUDE con $((($env:INCLUDE -split ';') | Where-Object { $_ }).Count) rutas"

# --- PATH: añadir bin de FFmpeg ---
if ($env:Path -notlike "*$ffmpegBin*") {
    $env:Path += ";$ffmpegBin"
}
Write-Ok "PATH extendido con $ffmpegBin"

# --- VCPKG_ROOT y CMAKE_GENERATOR persistentes ---
[Environment]::SetEnvironmentVariable("VCPKG_ROOT", $VcpkgRoot, "User")
$env:VCPKG_ROOT = $VcpkgRoot
Write-Ok "VCPKG_ROOT = $VcpkgRoot"

[Environment]::SetEnvironmentVariable("CMAKE_GENERATOR", $CmakeGen, "User")
$env:CMAKE_GENERATOR = $CmakeGen
Write-Ok "CMAKE_GENERATOR = $CmakeGen"

# --- BINDGEN_EXTRA_CLANG_ARGS: la clave para que bindgen genere campos ---
# --driver-mode=cl hace que libclang use los mismos defines que cl.exe
$clangArgs = @(
    "-fms-extensions",
    "-fms-compatibility",
    "-fdelayed-template-parsing",
    "-D_CRT_SECURE_NO_WARNINGS",
    "-D_WIN32",
    "-D_WIN64",
    "-D_WIN32_WINNT=0x0A00",
    "-DHAVE_UNISTD_H=0",
    "-I`"$ffmpegInclude`""
)

# Entre comillas: "C:\Program Files (x86)\Windows Kits\..." tiene espacio,
# y BINDGEN_EXTRA_CLANG_ARGS se parte por espacios en blanco tal cual —
# sin comillas, esa ruta se corta en pedazos sueltos y libclang tira
# "Invalid flag syntax".
if ($msvcInc) { $clangArgs += "-I`"$msvcInc`"" }
foreach ($sdkPath in $sdkPaths) { $clangArgs += "-I`"$sdkPath`"" }

$env:BINDGEN_EXTRA_CLANG_ARGS = $clangArgs -join " "
Write-Ok "BINDGEN_EXTRA_CLANG_ARGS configurado"
Write-Host "      $env:BINDGEN_EXTRA_CLANG_ARGS" -ForegroundColor Gray

# ============================================================
# 8. Compilar el proyecto
# ============================================================
Write-Step "Compilando proyecto Rust ($Profile)"

Push-Location $ProjectRoot
try {
    if ($Profile -eq "release") {
        Invoke-Native cargo build --release
    } else {
        Invoke-Native cargo build
    }
    Write-Ok "Compilación exitosa"
} finally {
    Pop-Location
}

# ============================================================
# 9. Copiar DLLs junto al .exe
# ============================================================
Write-Step "Copiando DLLs junto al ejecutable"

$outDir = Join-Path $ProjectRoot "target\$Profile"
if (-not (Test-Path $outDir)) { throw "No existe $outDir" }

$dllSources = @()
if ($useManualFfmpeg) {
    $dllSources += "$ManualFfmpeg\bin"
} else {
    $dllSources += "$installedDir\bin"
}

$patterns = @("av*.dll", "sw*.dll", "sdl2*.dll", "SDL2*.dll", "postproc*.dll")
$copied = 0
foreach ($src in $dllSources) {
    if (-not (Test-Path $src)) { continue }
    foreach ($p in $patterns) {
        Get-ChildItem -Path $src -Filter $p -ErrorAction SilentlyContinue | ForEach-Object {
            Copy-Item $_.FullName $outDir -Force
            $copied++
        }
    }
}
Write-Ok "$copied DLLs copiadas a $outDir"

# ============================================================
# 10. Empaquetar (opcional)
# ============================================================
if ($Package) {
    Write-Step "Empaquetando ZIP portable"

    $exePath = Join-Path $outDir "$BinaryName.exe"
    if (-not (Test-Path $exePath)) {
        throw "No se encontró $exePath. ¿El nombre del binario es '$BinaryName'?"
    }

    $zipName = "$BinaryName-$Profile-portable.zip"
    $zipPath = Join-Path $ProjectRoot $zipName
    if (Test-Path $zipPath) { Remove-Item $zipPath -Force }

    $stageDir = Join-Path $env:TEMP "$BinaryName-package"
    if (Test-Path $stageDir) { Remove-Item $stageDir -Recurse -Force }
    New-Item -ItemType Directory -Path $stageDir | Out-Null

    Copy-Item $exePath $stageDir
    Get-ChildItem -Path $outDir -Filter "*.dll" -ErrorAction SilentlyContinue | ForEach-Object {
        Copy-Item $_.FullName $stageDir
    }

    Compress-Archive -Path "$stageDir\*" -DestinationPath $zipPath -Force
    Remove-Item $stageDir -Recurse -Force
    Write-Ok "ZIP creado: $zipPath"
}

# ============================================================
# 11. Resumen
# ============================================================
Write-Step "¡Listo!"

$finalExe = Join-Path $outDir "$BinaryName.exe"
Write-Host ""
Write-Host "  Ejecutable:   $finalExe" -ForegroundColor Green
Write-Host "  Perfil:       $Profile"
Write-Host "  FFmpeg:       $(if ($useManualFfmpeg) { "$ManualFfmpeg (manual)" } else { "$installedDir (vcpkg)" })"
Write-Host "  VCPKG_ROOT:   $env:VCPKG_ROOT"
Write-Host "  FFMPEG_DIR:   $env:FFMPEG_DIR"
Write-Host "  LIBCLANG:     $env:LIBCLANG_PATH"
Write-Host "  NASM:         $env:ASM_NASM"
Write-Host "  CMAKE_GEN:    $env:CMAKE_GENERATOR"
Write-Host ""
Write-Host "  Para ejecutar:" -ForegroundColor Cyan
Write-Host "      cd `"$outDir`""
Write-Host "      .\$BinaryName.exe"
Write-Host ""