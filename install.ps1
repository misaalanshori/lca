# LCA installer and updater - https://github.com/misaalanshori/lca
#
#   irm https://raw.githubusercontent.com/misaalanshori/lca/main/install.ps1 | iex
#
# Spec: docs/installation.md. Decision: docs/adr/0040-install-and-update.md.
# Requirements: FR-INSTALL-1..9. Tests: tests/install/test_install_ps1.ps1.
#
# Windows PowerShell 5.1 and pwsh - no PS7-only syntax. The main body is
# guarded so the file can be dot-sourced for its functions (the test suite
# asserts on Join-UserPath / Get-AssetName directly).
# Exit codes: 0 ok, 1 fetch/verify/write failure, 2 unsupported architecture.
# A script file exits with the code; an in-memory run (`iex`,
# scriptblock::Create) sets $LASTEXITCODE and returns to the caller (gh #26).

[CmdletBinding()]
param(
  [string]$Version,
  [string]$InstallDir,
  [switch]$NoPath,
  [switch]$Uninstall,
  [switch]$Unstable,
  [string]$BaseUrl
)

# Does a script file back this run? `exit` is safe only then: in an
# in-memory invocation there is no file, the "current script" is the
# caller's session, and `exit` would end the host (gh #26).
$LcaInMemory = [string]::IsNullOrEmpty($PSCommandPath)

# --- functions (no side effects at definition) -------------------------------

# The platform map. An architecture with no release asset is an error, never
# a guess (FR-INSTALL-7).
function Get-AssetName {
  param([string]$Arch)
  switch -Regex ([string]$Arch) {
    '^AMD64$' { return 'lca-x86_64-pc-windows-msvc.exe' }
    '^ARM64$' { return 'lca-aarch64-pc-windows-msvc.exe' }
    default {
      $shown = [string]$Arch
      if (-not $shown) { $shown = '<empty PROCESSOR_ARCHITECTURE>' }
      throw "unsupported architecture: $shown - no release asset has that name."
    }
  }
}

# Append $Dir to a PATH value without duplicating it, dropping empty entries
# and preserving every other entry in order (FR-INSTALL-9). Case-insensitive,
# because the filesystem is.
function Join-UserPath {
  param([string]$Current, [Parameter(Mandatory = $true)][string]$Dir)
  $entries = @()
  if ($Current) { $entries = @($Current -split ';' | Where-Object { $_ -ne '' }) }
  $wanted = $Dir.TrimEnd('\')
  $found = $false
  foreach ($entry in $entries) {
    if ($entry.TrimEnd('\') -ieq $wanted) { $found = $true; break }
  }
  if (-not $found) { $entries += $Dir }
  return ($entries -join ';')
}

# The inverse: drop $Dir, change nothing else (FR-INSTALL-5's Windows half).
function Remove-UserPathEntry {
  param([string]$Current, [Parameter(Mandatory = $true)][string]$Dir)
  if (-not $Current) { return $Current }
  $wanted = $Dir.TrimEnd('\')
  $entries = @($Current -split ';' | Where-Object { $_ -ne '' -and $_.TrimEnd('\') -ine $wanted })
  return ($entries -join ';')
}

# Read the raw user Path (unexpanded) and its value kind, so a REG_EXPAND_SZ
# value survives a round trip as REG_EXPAND_SZ (FR-INSTALL-9).
function Get-UserPathState {
  $state = [pscustomobject]@{ Raw = $null; Kind = $null }
  $key = $null
  try { $key = [Microsoft.Win32.Registry]::CurrentUser.OpenSubKey('Environment', $false) } catch { $key = $null }
  if ($null -ne $key) {
    try {
      $raw = $key.GetValue('Path', $null, [Microsoft.Win32.RegistryValueOptions]::DoNotExpandEnvironmentNames)
      if ($null -ne $raw) {
        $state.Raw = [string]$raw
        $state.Kind = $key.GetValueKind('Path')
      }
    } catch { }
    $key.Close()
  }
  return $state
}

function Set-UserPathState {
  param($State)
  if ($null -eq $State -or $null -eq $State.Raw) { return }
  $key = $null
  try { $key = [Microsoft.Win32.Registry]::CurrentUser.OpenSubKey('Environment', $true) } catch { $key = $null }
  if ($null -eq $key) {
    [Environment]::SetEnvironmentVariable('Path', $State.Raw, 'User')
    return
  }
  $kind = $State.Kind
  if ($null -eq $kind) { $kind = [Microsoft.Win32.RegistryValueKind]::String }
  $key.SetValue('Path', $State.Raw, $kind)
  $key.Close()
}

# Is the directory already reachable? User or machine Path, case-insensitive
# (FR-INSTALL-3: already on PATH means no edit at all).
function Test-PathHas {
  param([Parameter(Mandatory = $true)][string]$Dir)
  $state = Get-UserPathState
  $values = @()
  if ($state.Raw) { $values += $state.Raw -split ';' }
  try {
    $machine = [Environment]::GetEnvironmentVariable('Path', 'Machine')
    if ($machine) { $values += $machine -split ';' }
  } catch { }
  $wanted = $Dir.TrimEnd('\')
  foreach ($value in $values) {
    if ($value -and $value.TrimEnd('\') -ieq $wanted) { return $true }
  }
  return $false
}

# Explorer caches the environment at logon; without this broadcast a terminal
# opened after the install still sees the old Path.
function Send-PathChange {
  try {
    if (-not ('LcaNative.EnvironmentChange' -as [type])) {
      Add-Type -Namespace LcaNative -Name EnvironmentChange -MemberDefinition @'
[DllImport("user32.dll", SetLastError = true, CharSet = CharSet.Auto)]
public static extern IntPtr SendMessageTimeout(IntPtr hWnd, uint Msg, UIntPtr wParam, string lParam, uint fuFlags, uint uTimeout, out UIntPtr lpdwResult);
'@
    }
    [UIntPtr]$result = [UIntPtr]::Zero
    [void][LcaNative.EnvironmentChange]::SendMessageTimeout(
      [IntPtr]0xffff, 0x1A, [UIntPtr]::Zero, 'Environment', 2, 5000, [ref]$result)
  } catch { }  # best effort: the new terminal usually picks the path up anyway
}

# The installed binary's first version line, or nothing - this executes the
# freshly verified binary, which is what the update report needs (FR-INSTALL-4).
function Read-Version {
  param([string]$Path)
  if (-not $Path -or -not (Test-Path -LiteralPath $Path)) { return $null }
  try {
    $line = & $Path --version 2>$null | Select-Object -First 1
    if (-not $line) { return $null }
    $text = ([string]$line).Trim()
    # clap prints "lca 0.6.0" for --version; drop the name once here so
    # every message downstream can say "lca <ver>" without saying it twice.
    if ($text.StartsWith('lca ')) { $text = $text.Substring(4) }
    return $text
  } catch { }
  return $null
}

# One base, two patterns: <base>/latest/download/<name> and
# <base>/download/v<V>/<name> (FR-INSTALL-6, FR-INSTALL-8).
function Get-ReleaseItem {
  param([Parameter(Mandatory = $true)][string]$Name)
  # The unstable line: one fixed directory, replaced per green commit
  # (ADR-0043); the checksums live beside the binaries, so verification
  # runs unchanged.
  $segments = if ($script:LcaUnstable) {
    "download/unstable/$($Name)"
  } elseif ($script:LcaVersion) {
    "download/v$($script:LcaVersion)/$($Name)"
  } else {
    "latest/download/$($Name)"
  }
  if ($script:LcaLocalBase) {
    return [IO.Path]::Combine($script:LcaBaseRoot, ($segments -replace '/', [IO.Path]::DirectorySeparatorChar))
  }
  return "$($script:LcaBaseRoot)/$segments"
}

function Resolve-ReleaseBase {
  param([Parameter(Mandatory = $true)][string]$Raw)
  if ($Raw -like 'file://*') {
    return @{ Local = $true; Root = ([Uri]$Raw).LocalPath }
  }
  if (Test-Path -LiteralPath $Raw -PathType Container) {
    return @{ Local = $true; Root = $Raw }
  }
  return @{ Local = $false; Root = $Raw.TrimEnd('/') }
}

function Get-RemoteFile {
  param([string]$Source, [Parameter(Mandatory = $true)][string]$Destination)
  Write-Verbose "GET $Source"
  if ($script:LcaLocalBase) {
    if (-not (Test-Path -LiteralPath $Source -PathType Leaf)) {
      throw "not found: $Source"
    }
    Copy-Item -LiteralPath $Source -Destination $Destination -Force
    return
  }
  try {
    try {
      [Net.ServicePointManager]::SecurityProtocol = `
        [Net.ServicePointManager]::SecurityProtocol -bor [Net.SecurityProtocolType]::Tls12
    } catch { }  # Windows PowerShell 5.1 defaults are policy-driven; force TLS 1.2
    Invoke-WebRequest -Uri $Source -OutFile $Destination -UseBasicParsing | Out-Null
  } catch {
    throw "cannot fetch ${Source}: $($_.Exception.Message)"
  }
}

function Write-Err([string]$Message) {
  Write-Output "error: $Message"
}

# Finish the run: publish the documented code, then leave - `exit` when this
# is a script file (which is what hands `& .\install.ps1` and `pwsh -File`
# their `$LASTEXITCODE`), and nothing when it is not, so the caller's
# `iex` / scriptblock keeps its session. Every call site follows this with a
# `return`, which ends the main body in the in-memory case and is unreachable
# in the file case.
function Complete-Install {
  param([int]$Code)
  $global:LASTEXITCODE = $Code
  if (-not $LcaInMemory) { exit $Code }
}

# --- main --------------------------------------------------------------------

if ($MyInvocation.InvocationName -eq '.') {
  return  # dot-sourced for the functions; nothing else runs
}

# The install runs as one function, and that is load-bearing (gh #26).
# `Invoke-Expression` evaluates its string in the CALLER's current scope
# (Microsoft: "expressions are evaluated and run in the current scope"),
# so a top-level `return` would return from the caller's own scope and
# swallow the rest of the command the installer was pasted into. Inside a
# function, `return` stops this body and nothing else; `Complete-Install`
# still calls `exit` when a script file backs the run, which ends the whole
# script and hands the caller its `$LASTEXITCODE`.
function Invoke-LcaInstall {
  $ErrorActionPreference = 'Stop'

  $install = $InstallDir
  if (-not $install) {
    if ($env:LCA_INSTALL_DIR) { $install = $env:LCA_INSTALL_DIR }
    else { $install = Join-Path $env:LOCALAPPDATA 'lca\bin' }
  }
  try { $install = [IO.Path]::GetFullPath($install) } catch { }

  $bin = Join-Path $install 'lca.exe'

  try {
    # Uninstall: no network, no platform detection (FR-INSTALL-5).
    if ($Uninstall) {
      $removed = $false
      if (Test-Path -LiteralPath $bin) {
        Remove-Item -LiteralPath $bin -Force
        Write-Output "removed binary: $bin"
        $removed = $true
      }
      if (-not $NoPath) {
        $state = Get-UserPathState
        if ($state.Raw) {
          $trimmed = Remove-UserPathEntry -Current $state.Raw -Dir $install
          if ($trimmed -ne $state.Raw) {
            $state.Raw = $trimmed
            Set-UserPathState $state
            Send-PathChange
            Write-Output "removed PATH entry: $install"
            $removed = $true
          }
        }
      }
      if (-not $removed) {
        Write-Output "nothing to remove: no lca.exe at $bin and no installer PATH entry"
      }
      Complete-Install 0; return
    }

    $arch = $env:PROCESSOR_ARCHITECTURE
    try {
      $asset = Get-AssetName -Arch $arch
    } catch {
      Write-Err $_.Exception.Message
      Write-Output 'On Linux or macOS, use the POSIX installer instead:'
      Write-Output '  curl -fsSL https://raw.githubusercontent.com/misaalanshori/lca/main/install.sh | sh'
      Complete-Install 2; return
    }

    $baseRaw = $BaseUrl
    if (-not $baseRaw) {
      if ($env:LCA_BASE_URL) { $baseRaw = $env:LCA_BASE_URL }
      else { $baseRaw = 'https://github.com/misaalanshori/lca/releases' }
    }
    $resolved = Resolve-ReleaseBase -Raw $baseRaw
    $script:LcaLocalBase = [bool]$resolved.Local
    $script:LcaBaseRoot = [string]$resolved.Root
    $script:LcaVersion = ''
    if ($Version) { $script:LcaVersion = $Version.TrimStart('v') }
    $script:LcaUnstable = [bool]$Unstable
    if ($script:LcaUnstable -and $script:LcaVersion) {
      Write-Output 'error: -Unstable and -Version cannot be combined: the unstable line is latest-only'
      Complete-Install 2; return
    }

    $temp = Join-Path ([IO.Path]::GetTempPath()) ('lca-install-' + [Guid]::NewGuid().ToString('N'))
    New-Item -ItemType Directory -Force -Path $temp | Out-Null

    $sumFile = Join-Path $temp 'artifacts.sha256'
    $assetFile = Join-Path $temp $asset

    try {
      Get-RemoteFile -Source (Get-ReleaseItem -Name 'artifacts.sha256') -Destination $sumFile
      Get-RemoteFile -Source (Get-ReleaseItem -Name $asset) -Destination $assetFile
    } catch {
      Remove-Item -LiteralPath $temp -Recurse -Force -ErrorAction SilentlyContinue
      Write-Err $_.Exception.Message
      Complete-Install 1; return
    }

    # artifacts.sha256: "<hash><two spaces><name>", case-insensitive compare.
    $expected = $null
    foreach ($line in (Get-Content -LiteralPath $sumFile)) {
      $parts = ($line.Trim() -split '\s+')
      if ($parts.Count -ge 2 -and ($parts[1].TrimStart('*') -ieq $asset)) {
        $expected = $parts[0].ToLowerInvariant()
        break
      }
    }
    if (-not $expected) {
      Remove-Item -LiteralPath $temp -Recurse -Force -ErrorAction SilentlyContinue
      Write-Err "artifacts.sha256 has no entry for $asset"
      Complete-Install 1; return
    }
    $actual = (Get-FileHash -Algorithm SHA256 -LiteralPath $assetFile).Hash.ToLowerInvariant()
    if ($expected -ne $actual) {
      Remove-Item -LiteralPath $temp -Recurse -Force -ErrorAction SilentlyContinue
      Write-Output 'error: checksum mismatch'
      Write-Output "  expected: $expected"
      Write-Output "  actual:   $actual"
      Complete-Install 1; return
    }

    $old = Read-Version $bin

    New-Item -ItemType Directory -Force -Path $install | Out-Null
    try {
      Move-Item -LiteralPath $assetFile -Destination $bin -Force
    } catch {
      try {
        Copy-Item -LiteralPath $assetFile -Destination $bin -Force
        Remove-Item -LiteralPath $assetFile -Force -ErrorAction SilentlyContinue
      } catch {
        Remove-Item -LiteralPath $temp -Recurse -Force -ErrorAction SilentlyContinue
        Write-Err "cannot place the binary at $bin"
        Complete-Install 1; return
      }
    }
    Remove-Item -LiteralPath $temp -Recurse -Force -ErrorAction SilentlyContinue

    $new = Read-Version $bin
    if (-not $new) { $new = 'unknown' }

    Write-Output "installed lca $new to $bin"
    if ($old) { Write-Output "lca $old -> $new" }

    if ($NoPath) {
      Write-Output "PATH: left alone (-NoPath). Open a new terminal, or add $install to PATH."
      Complete-Install 0; return
    }
    if (Test-PathHas -Dir $install) {
      Write-Output "PATH: $install is already on PATH; no change."
      Complete-Install 0; return
    }
    $state = Get-UserPathState
    $joined = Join-UserPath -Current $state.Raw -Dir $install
    $state.Raw = $joined
    if ($null -eq $state.Kind) { $state.Kind = [Microsoft.Win32.RegistryValueKind]::String }
    Set-UserPathState $state
    Send-PathChange
    Write-Output "PATH: added $install to the user Path"
    Write-Output 'Restart your shell (open a new terminal) to use lca.'
    Complete-Install 0; return
  } catch {
    if ($temp) { Remove-Item -LiteralPath $temp -Recurse -Force -ErrorAction SilentlyContinue }
    Write-Err $_.Exception.Message
    Complete-Install 1; return
  }
}

Invoke-LcaInstall
