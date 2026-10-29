# Installs the lock binary from the latest GitHub release.
#
#   irm https://raw.githubusercontent.com/dariodaddamio/lock/main/install.ps1 | iex
#
# $env:LOCK_VERSION picks a release tag, such as v0.1.0. $env:LOCK_INSTALL_DIR picks
# the directory, which is %LOCALAPPDATA%\Programs\lock by default. Only the default
# directory is added to your user PATH.
$ErrorActionPreference = 'Stop'

$repo = 'dariodaddamio/lock'
$arch = if ($env:PROCESSOR_ARCHITECTURE -eq 'ARM64') { 'aarch64' } else { 'x86_64' }
$target = "$arch-pc-windows-msvc"
$release = if ($env:LOCK_VERSION) { "download/$env:LOCK_VERSION" } else { 'latest/download' }
$url = "https://github.com/$repo/releases/$release/lock-$target.zip"
$custom = [bool]$env:LOCK_INSTALL_DIR
$dir = if ($custom) { $env:LOCK_INSTALL_DIR } else { Join-Path $env:LOCALAPPDATA 'Programs\lock' }

$zip = Join-Path ([IO.Path]::GetTempPath()) ("lock-" + [guid]::NewGuid() + '.zip')
Write-Host "Downloading $url"
Invoke-WebRequest -Uri $url -OutFile $zip -UseBasicParsing
New-Item -ItemType Directory -Force -Path $dir | Out-Null
Expand-Archive -Path $zip -DestinationPath $dir -Force
Remove-Item $zip
$version = & (Join-Path $dir 'lock.exe') --version
Write-Host "Installed $version to $dir"

if (-not $custom) {
    $userPath = [Environment]::GetEnvironmentVariable('Path', 'User')
    if (-not (($userPath -split ';') -contains $dir)) {
        [Environment]::SetEnvironmentVariable('Path', ($userPath.TrimEnd(';') + ";$dir"), 'User')
        $env:Path += ";$dir"
        Write-Host "Added $dir to your PATH. Open a new terminal before you use lock there."
    }
}
Write-Host "Next, run 'lock install' in the root of each repository that you want to protect."
