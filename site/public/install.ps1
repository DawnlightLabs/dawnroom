# Dawnroom installer for Windows (x64). Per user, no admin, no telemetry.
#   irm https://dawnroom.dawnlightlabs.com/install.ps1 | iex
#   & ([scriptblock]::Create((irm https://dawnroom.dawnlightlabs.com/install.ps1))) --cli   (engine + CLI only)
# It downloads the latest release from GitHub (DawnlightLabs/dawnroom), checks it against the
# release's SHA256SUMS, then runs the same installer as the download page, silently (Studio, Start
# menu, tray, uninstall entry, `dawnroom` on your PATH). With --cli it only puts the engine and its
# model list in %LOCALAPPDATA%\Programs\Dawnroom and adds that folder to your PATH.
# Your models and images live in %LOCALAPPDATA%\Dawnroom and are never touched. Re-run to update.
$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'   # Invoke-WebRequest is many times slower with its progress bar
[Net.ServicePointManager]::SecurityProtocol = [Net.SecurityProtocolType]::Tls12

& {
  $releases = if ($env:DAWNROOM_RELEASES) { $env:DAWNROOM_RELEASES } else { 'https://api.github.com/repos/DawnlightLabs/dawnroom/releases/latest' }
  $cliOnly = $args -contains '--cli'
  $dir = Join-Path $env:LOCALAPPDATA 'Programs\Dawnroom'
  # Every failure ends here: one plain line, the temp folder removed, nothing half-installed.
  function Fail($why) { throw [Exception]::new($why) }

  $tmp = $null
  try {
  if ($env:PROCESSOR_ARCHITECTURE -ne 'AMD64') { Fail "only x64 Windows is built so far ($env:PROCESSOR_ARCHITECTURE isn't yet)." }

  try { $rel = Invoke-RestMethod $releases } catch { Fail "couldn't reach $releases." }
  $version = $rel.tag_name -replace '^v', ''
  $tmp = Join-Path ([IO.Path]::GetTempPath()) "dawnroom-install-$([guid]::NewGuid().ToString('N'))"
  New-Item -ItemType Directory $tmp | Out-Null
    # Download a release file and check it against SHA256SUMS before anything uses it.
    function Get-Checked($name) {
      $asset = $rel.assets | Where-Object { $_.name -eq $name } | Select-Object -First 1
      if (-not $asset) { Fail "the release has no $name." }
      $out = Join-Path $tmp $name
      Write-Host "Downloading $name ($([math]::Round($asset.size / 1MB)) MB)..."
      try { Invoke-WebRequest $asset.browser_download_url -OutFile $out -UseBasicParsing } catch { Fail "the download of $name broke off." }
      $line = $sums | Where-Object { $_ -match "^([0-9a-fA-F]{64}) [ *]?$([regex]::Escape($name))$" } | Select-Object -First 1
      if (-not $line) { Fail "$name isn't listed in SHA256SUMS." }
      if ((Get-FileHash $out -Algorithm SHA256).Hash -ne $line.Substring(0, 64).ToUpper()) { Fail "$name doesn't match its checksum (download damaged or tampered with). Nothing was changed." }
      $out
    }
    $sumsAsset = $rel.assets | Where-Object { $_.name -eq 'SHA256SUMS' } | Select-Object -First 1
    if (-not $sumsAsset) { Fail 'the release has no SHA256SUMS.' }
    # Saved to a file: GitHub serves it as bytes (application/octet-stream), not text.
    try { Invoke-WebRequest $sumsAsset.browser_download_url -OutFile "$tmp\SHA256SUMS" -UseBasicParsing } catch { Fail "couldn't download SHA256SUMS." }
    $sums = Get-Content "$tmp\SHA256SUMS" | ForEach-Object { $_.Trim() }

    # A running engine lets go of its files first.
    if (Test-Path "$dir\dawnroom.exe") { & "$dir\dawnroom.exe" stop *> $null }

    if ($cliOnly) {
      $zip = Get-Checked "dawnroom-$version-windows-x64.zip"
      Expand-Archive $zip (Join-Path $tmp 'new')
      if (-not (Test-Path "$tmp\new\dawnroom.exe")) { Fail 'the archive has no dawnroom.exe.' }
      New-Item -ItemType Directory -Force $dir | Out-Null
      Move-Item -Force "$tmp\new\dawnroom.exe" "$dir\dawnroom.exe"
      if (Test-Path "$dir\registry") { Remove-Item -Recurse -Force "$dir\registry" }
      Move-Item "$tmp\new\registry" "$dir\registry"
      # The install folder on the user's PATH, other entries kept as written (%VARS% unexpanded).
      $key = [Microsoft.Win32.Registry]::CurrentUser.CreateSubKey('Environment')
      $path = [string]$key.GetValue('Path', '', 'DoNotExpandEnvironmentNames')
      $parts = @($path -split ';' | Where-Object { $_ -and ($_.TrimEnd('\') -ne $dir) }) + $dir
      $key.SetValue('Path', ($parts -join ';'), 'ExpandString')
      [Environment]::SetEnvironmentVariable('DAWNROOM_PATH_REFRESH', $null, 'User')   # tells Windows the environment changed
      Write-Host "Dawnroom $version installed. Run ``dawnroom`` in a new terminal."
    } else {
      $setup = Get-Checked "Dawnroom_${version}_x64-setup.exe"
      $p = Start-Process $setup -ArgumentList '/S' -Wait -PassThru
      if ($p.ExitCode -ne 0) { Fail "the installer stopped with code $($p.ExitCode)." }
      Write-Host "Dawnroom $version installed. Run ``dawnroom`` in a new terminal or open Dawnroom from the Start menu."
    }
  } catch {
    Write-Host "Dawnroom wasn't installed: $($_.Exception.Message)" -ForegroundColor Red
  } finally {
    if ($tmp) { Remove-Item -Recurse -Force $tmp -ErrorAction SilentlyContinue }
  }
} @args
