param(
    [ValidateSet("Debug", "Release")]
    [string]$Configuration = "Release"
)

$ErrorActionPreference = "Stop"
$workspaceRoot = Split-Path -Parent $PSScriptRoot
$softcamRoot = Join-Path $workspaceRoot "native\softcam"
$mfCameraRoot = Join-Path $workspaceRoot "native\windows-camera-reference\Samples\VirtualCamera"
$mfManagerRoot = Join-Path $workspaceRoot "native\mfvcam-manager"
$dshowManagerRoot = Join-Path $workspaceRoot "native\dshow-manager"
$vswhere = Join-Path ${env:ProgramFiles(x86)} "Microsoft Visual Studio\Installer\vswhere.exe"

if (-not (Test-Path -LiteralPath $vswhere)) {
    throw "Visual Studio Installer의 vswhere.exe를 찾을 수 없습니다."
}

$msbuild = & $vswhere -latest -products * -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -find "MSBuild\**\Bin\MSBuild.exe" | Select-Object -First 1
if (-not $msbuild) {
    throw "Visual Studio C++ 빌드 도구를 찾을 수 없습니다."
}

$savedPath = $env:PATH
Remove-Item Env:PATH -ErrorAction SilentlyContinue

try {
    foreach ($platform in @("x64", "Win32")) {
        & $msbuild `
            (Join-Path $softcamRoot "src\softcam\softcam.vcxproj") `
            /m `
            "/p:Configuration=$Configuration" `
            "/p:Platform=$platform" `
            "/p:SolutionDir=$softcamRoot\"
        if ($LASTEXITCODE -ne 0) { throw "DirectShow softcam $platform build failed." }

        & $msbuild `
            (Join-Path $dshowManagerRoot "dshow_manager.vcxproj") `
            /m `
            "/p:Configuration=$Configuration" `
            "/p:Platform=$platform"
        if ($LASTEXITCODE -ne 0) { throw "DirectShow manager $platform build failed." }
    }

    & $msbuild `
        (Join-Path $mfCameraRoot "VirtualCameraMediaSource\VirtualCameraMediaSource.vcxproj") `
        /m `
        "/p:Configuration=$Configuration" `
        "/p:Platform=x64" `
        "/p:SolutionDir=$mfCameraRoot\"
    if ($LASTEXITCODE -ne 0) { throw "Media Foundation virtual camera build failed." }

    & $msbuild `
        (Join-Path $mfManagerRoot "mfvcam_manager.vcxproj") `
        /m `
        "/p:Configuration=$Configuration" `
        "/p:Platform=x64"
    if ($LASTEXITCODE -ne 0) { throw "Media Foundation virtual camera manager build failed." }
}
finally {
    $env:PATH = $savedPath
}

Write-Host "Media Foundation and DirectShow camera components built successfully."
