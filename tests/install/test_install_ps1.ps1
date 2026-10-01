# The install.ps1 suite (docs/testing-plan.md section 15).
#
# Verifies: FR-INSTALL-2 FR-INSTALL-3 FR-INSTALL-5 FR-INSTALL-6 FR-INSTALL-7
# Verifies: FR-INSTALL-8 FR-INSTALL-9
# Verifies: FR-INSTALL-10
#
# Hermetic: fixtures in a temp directory, -BaseUrl pointed at them (the
# fetch seam, FR-INSTALL-6), -InstallDir under the same temp root, and
# -NoPath everywhere except the single registry round-trip case, which
# restores the runner's original user Path in a finally block.
#
# Run under both shells the requirement names:
#   powershell -NoProfile -File tests\install\test_install_ps1.ps1
#   pwsh        -NoProfile -File tests\install\test_install_ps1.ps1

$ErrorActionPreference = 'Continue'
$ProgressPreference = 'SilentlyContinue'

$Repo = Split-Path -Parent (Split-Path -Parent $PSScriptRoot)
$Installer = Join-Path $Repo 'install.ps1'
if (-not (Test-Path -LiteralPath $Installer)) {
  Write-Host "FAIL: $Installer not found"
  exit 1
}

$script:Passed = 0
$script:Root = $null
$script:LastOutput = ''
$script:LastCode = 0

function Fail([string]$Message) {
  Write-Host "FAIL: $Message"
  if ($script:LastOutput) { $script:LastOutput -split "`n" | ForEach-Object { Write-Host "  | $_" } }
  exit 1
}

function Assert([bool]$Condition, [string]$Message) {
  if (-not $Condition) { Fail $Message }
}

function Pass([string]$Message) {
  $script:Passed++
  Write-Host "ok $($script:Passed) - $Message"
}

$script:AssetNames = @('lca-x86_64-pc-windows-msvc.exe', 'lca-aarch64-pc-windows-msvc.exe')

# A real executable that answers --version, so the installer's
# read-the-old-version-before-replacing path has something to read.
# LCA_TEST_FIXTURE_EXE overrides the search (used for a non-Windows dry run
# of the registry-free parts of this suite).
function Get-FixtureExe {
  if ($env:LCA_TEST_FIXTURE_EXE -and (Test-Path -LiteralPath $env:LCA_TEST_FIXTURE_EXE)) {
    return $env:LCA_TEST_FIXTURE_EXE
  }
  $candidates = @()
  if ($env:WINDIR) { $candidates += (Join-Path $env:WINDIR 'System32\curl.exe') }
  $candidates += 'C:\Program Files\Git\usr\bin\curl.exe'
  if ($env:WINDIR) { $candidates += (Join-Path $env:WINDIR 'System32\where.exe') }
  foreach ($candidate in $candidates) {
    if ($candidate -and (Test-Path -LiteralPath $candidate)) { return $candidate }
  }
  Fail 'no fixture executable found (curl.exe / where.exe; set LCA_TEST_FIXTURE_EXE to override)'
}

function Write-Checksums([string]$Dir) {
  $lines = @()
  foreach ($name in $script:AssetNames) {
    $hash = (Get-FileHash -Algorithm SHA256 -LiteralPath (Join-Path $Dir $name)).Hash.ToLowerInvariant()
    $lines += ('{0}  {1}' -f $hash, $name)
  }
  Set-Content -LiteralPath (Join-Path $Dir 'artifacts.sha256') -Value $lines -Encoding Ascii
}

# The pinned directory's assets carry a marker the latest ones do not, so
# "-Version installed the right bytes" is an assertion on content.
function New-Fixture {
  $latest = Join-Path $script:Root 'release\latest\download'
  $pinned = Join-Path $script:Root 'release\download\v9.9.9'
  New-Item -ItemType Directory -Force -Path $latest, $pinned | Out-Null
  $exe = Get-FixtureExe
  foreach ($name in $script:AssetNames) {
    Copy-Item -LiteralPath $exe -Destination (Join-Path $latest $name) -Force
    Copy-Item -LiteralPath $exe -Destination (Join-Path $pinned $name) -Force
    Add-Content -LiteralPath (Join-Path $pinned $name) -Value "pinned-$name" -Encoding Ascii
  }
  Write-Checksums $latest
  Write-Checksums $pinned
}

function Setup {
  $script:Root = Join-Path ([IO.Path]::GetTempPath()) ('lca-install-test-' + [Guid]::NewGuid().ToString('N'))
  New-Item -ItemType Directory -Force -Path $script:Root | Out-Null
  $script:InstallDir = Join-Path $script:Root 'bin'
  $script:BaseUrl = Join-Path $script:Root 'release'
  New-Fixture
}

function Teardown {
  if ($script:Root -and (Test-Path -LiteralPath $script:Root)) {
    Remove-Item -LiteralPath $script:Root -Recurse -Force -ErrorAction SilentlyContinue
  }
}

function Invoke-Installer([hashtable]$Params) {
  $script:LastOutput = (& $Installer @Params 2>&1 | Out-String)
  $script:LastCode = $LASTEXITCODE
}

# Dot-source for the function-level assertions: the main body is guarded
# against dot-sourcing, so this defines Join-UserPath, Get-AssetName, and the
# registry helpers without running an install.
. $Installer

# The registry is the one part of this contract that needs Windows. A host
# without it skips those rows with a named reason (testing plan section 14).
$script:HasRegistry = $true
try {
  $probe = [Microsoft.Win32.Registry]::CurrentUser.OpenSubKey('Environment', $false)
  if ($null -eq $probe) { $script:HasRegistry = $false } else { $probe.Close() }
} catch { $script:HasRegistry = $false }

function Skip([string]$Message, [string]$Reason = 'no user registry on this host') {
  Write-Host "# SKIP $Message (named skip: $Reason)"
}

# A tiny PE that prints the unstable line's version shape, so the old -> new
# report can be asserted with a real hash version. Windows PowerShell 5.1
# compiles it with Add-Type; a shell whose compiler refuses gets a named
# skip instead of a failed row.
function New-HashVersionExe {
  if (-not $script:Root) { return $null }
  $src = Join-Path $script:Root 'hashver.cs'
  $out = Join-Path $script:Root 'hashver.exe'
  try {
    [System.IO.File]::WriteAllText($src, 'using System; class LcaFixture { static void Main() { System.Console.WriteLine("lca 0.5.2.b6573049"); } }')
    Add-Type -Path $src -OutputAssembly $out -OutputType 'ConsoleApplication' -ErrorAction Stop
  } catch { return $null }
  try {
    if (Test-Path -LiteralPath $out) { return $out }
  } catch { }
  return $null
}

$ErrorActionPreference = 'Stop'
try {

  #############################################################################
  # FR-INSTALL-9: fresh install, verified, placed as lca.exe.
  #############################################################################
  Setup
  try {
    Invoke-Installer @{ BaseUrl = $script:BaseUrl; InstallDir = $script:InstallDir; NoPath = $true }
    Assert ($script:LastCode -eq 0) "fresh install: exit $($script:LastCode), expected 0"
    $bin = Join-Path $script:InstallDir 'lca.exe'
    Assert (Test-Path -LiteralPath $bin) 'fresh install: lca.exe missing'
    Assert ($script:LastOutput -match 'installed') 'fresh install: no confirmation message'
    Pass 'fresh install places lca.exe from the fixture release'
  } finally { Teardown }

  #############################################################################
  # FR-INSTALL-2 / FR-INSTALL-1: a tampered asset is refused and an existing
  # install is left byte-identical.
  #############################################################################
  Setup
  try {
    Invoke-Installer @{ BaseUrl = $script:BaseUrl; InstallDir = $script:InstallDir; NoPath = $true }
    Assert ($script:LastCode -eq 0) "baseline install: exit $($script:LastCode)"
    $bin = Join-Path $script:InstallDir 'lca.exe'
    $before = (Get-FileHash -Algorithm SHA256 -LiteralPath $bin).Hash

    $asset = Join-Path $script:BaseUrl 'latest\download\lca-x86_64-pc-windows-msvc.exe'
    Add-Content -LiteralPath $asset -Value 'tampered'
    Invoke-Installer @{ BaseUrl = $script:BaseUrl; InstallDir = $script:InstallDir; NoPath = $true }
    Assert ($script:LastCode -eq 1) "checksum mismatch: exit $($script:LastCode), expected 1"
    Assert ($script:LastOutput -match 'checksum') 'checksum mismatch: no refusal message'
    $after = (Get-FileHash -Algorithm SHA256 -LiteralPath $bin).Hash
    Assert ($before -eq $after) 'checksum mismatch: the installed binary was modified'
    Pass 'checksum mismatch refuses and leaves the installed binary intact'
  } finally { Teardown }

  #############################################################################
  # FR-INSTALL-1: a failed download leaves an existing install alone.
  #############################################################################
  Setup
  try {
    Invoke-Installer @{ BaseUrl = $script:BaseUrl; InstallDir = $script:InstallDir; NoPath = $true }
    Assert ($script:LastCode -eq 0) "baseline install: exit $($script:LastCode)"
    $bin = Join-Path $script:InstallDir 'lca.exe'
    $before = (Get-FileHash -Algorithm SHA256 -LiteralPath $bin).Hash
    $missing = Join-Path $script:Root 'does-not-exist'
    Invoke-Installer @{ BaseUrl = $missing; InstallDir = $script:InstallDir; NoPath = $true }
    Assert ($script:LastCode -eq 1) "failed download: exit $($script:LastCode), expected 1"
    $after = (Get-FileHash -Algorithm SHA256 -LiteralPath $bin).Hash
    Assert ($before -eq $after) 'failed download: the installed binary was modified'
    Pass 'failed download exits non-zero and leaves the installed binary intact'
  } finally { Teardown }

  #############################################################################
  # Verifies: FR-INSTALL-4 - a re-run over an existing install replaces it
  # through the same verify-then-move path and reports old -> new.
  #############################################################################
  Setup
  try {
    Invoke-Installer @{ BaseUrl = $script:BaseUrl; InstallDir = $script:InstallDir; NoPath = $true }
    Assert ($script:LastCode -eq 0) "first install: exit $($script:LastCode)"
    $bin = Join-Path $script:InstallDir 'lca.exe'
    if (-not (Read-Version $bin)) {
      Skip 'the re-run old -> new report' 'the fixture executable does not answer --version'
      Teardown
    } else {
      Invoke-Installer @{ BaseUrl = $script:BaseUrl; InstallDir = $script:InstallDir; NoPath = $true }
      Assert ($script:LastCode -eq 0) "re-run: exit $($script:LastCode)`n$($script:LastOutput)"
      Assert ($script:LastOutput -match 'lca .+ -> .+') "re-run: no 'lca <old> -> <new>' line:`n$($script:LastOutput)"
      Assert (Test-Path -LiteralPath $bin) 're-run: binary missing after the update'
      Pass 're-running updates and reports lca <old> -> <new>'
      Teardown
    }
  } finally { }

  #############################################################################
  # FR-INSTALL-8: -Version installs the pinned release, not the latest.
  #############################################################################
  Setup
  try {
    Invoke-Installer @{ BaseUrl = $script:BaseUrl; InstallDir = $script:InstallDir; NoPath = $true; Version = '9.9.9' }
    Assert ($script:LastCode -eq 0) "-Version: exit $($script:LastCode)`n$($script:LastOutput)"
    $bin = Join-Path $script:InstallDir 'lca.exe'
    $content = [IO.File]::ReadAllText($bin)
    Assert ($content -match 'pinned-lca-x86_64') '-Version: installed the latest asset instead of the pinned one'
    Pass '-Version 9.9.9 installs the pinned release'
  } finally { Teardown }

  #############################################################################
  # FR-INSTALL-5: -Uninstall removes the binary and says so.
  #############################################################################
  Setup
  try {
    Invoke-Installer @{ BaseUrl = $script:BaseUrl; InstallDir = $script:InstallDir; NoPath = $true }
    Assert ($script:LastCode -eq 0) "baseline install: exit $($script:LastCode)"
    $bin = Join-Path $script:InstallDir 'lca.exe'
    Invoke-Installer @{ InstallDir = $script:InstallDir; NoPath = $true; Uninstall = $true }
    Assert ($script:LastCode -eq 0) "-Uninstall: exit $($script:LastCode)"
    Assert (-not (Test-Path -LiteralPath $bin)) '-Uninstall: the binary is still there'
    Assert ($script:LastOutput -match 'remov') '-Uninstall: no removal report'
    Pass '-Uninstall removes the installed binary and reports it'
  } finally { Teardown }

  #############################################################################
  # FR-INSTALL-3: -NoPath leaves the user Path and its value kind untouched.
  #############################################################################
  Setup
  if (-not $script:HasRegistry) {
    Skip 'the user Path round-trip (-NoPath leaves it untouched)'
    Teardown
  } else {
  try {
    $before = Get-UserPathState
    Invoke-Installer @{ BaseUrl = $script:BaseUrl; InstallDir = $script:InstallDir; NoPath = $true }
    Assert ($script:LastCode -eq 0) "-NoPath: exit $($script:LastCode)"
    $after = Get-UserPathState
    Assert ($before.Raw -eq $after.Raw) '-NoPath: the user Path changed'
    Assert ($before.Kind -eq $after.Kind) '-NoPath: the Path value kind changed'
    Pass '-NoPath leaves the user Path untouched'
  } finally { Teardown }
  }

  #############################################################################
  # Unit assertions on the script's own functions (defined by the dot-source
  # above; the main body never ran).
  $existing = 'C:\Windows\System32;C:\tools\bin;C:\Users\runneradmin\AppData\Local\Temp'
  $joined = Join-UserPath -Current $existing -Dir 'C:\Users\runneradmin\AppData\Local\lca\bin'
  Assert (($joined -split ';').Count -eq 4) 'join: an entry was lost or duplicated'
  Assert ($joined.StartsWith('C:\Windows\System32;')) 'join: existing order was not preserved'
  $again = Join-UserPath -Current $joined -Dir 'C:\Users\runneradmin\AppData\Local\lca\bin'
  Assert ($again -eq $joined) 'join: a second join is not idempotent'
  $ci = Join-UserPath -Current 'C:\LCA\BIN;C:\Windows' -Dir 'c:\lca\bin'
  Assert (($ci -split ';').Count -eq 2) 'join: case-insensitive duplicate was added'
  $empty = Join-UserPath -Current $null -Dir 'C:\lca\bin'
  Assert ($empty -eq 'C:\lca\bin') 'join: null input produced something other than the dir'
  $clobber = Join-UserPath -Current 'C:\A;%SystemRoot%\system32;;C:\B' -Dir 'C:\lca\bin'
  Assert ($clobber -eq 'C:\A;%SystemRoot%\system32;C:\B;C:\lca\bin') 'join: entries were clobbered'
  Pass 'Join-UserPath is idempotent, order-preserving, and non-clobbering'

  Assert ((Get-AssetName -Arch 'AMD64') -eq 'lca-x86_64-pc-windows-msvc.exe') 'asset map: AMD64'
  Assert ((Get-AssetName -Arch 'ARM64') -eq 'lca-aarch64-pc-windows-msvc.exe') 'asset map: ARM64'
  $unsupported = 'mapped'
  try { $null = Get-AssetName -Arch 'sparc' } catch { $unsupported = 'unsupported' }
  Assert ($unsupported -eq 'unsupported') 'asset map: an unknown arch is not silently mapped'
  Pass 'the platform map names the right asset and refuses an unknown arch'

  # Read-Version must drop the leading "lca " clap prints exactly once: a
  # real one-liner run echoed "installed lca lca 0.5.2" before this guard.
  if ($env:WINDIR) {
    $fake = Join-Path ([IO.Path]::GetTempPath()) ('lca-fake-' + [Guid]::NewGuid().ToString('N') + '.cmd')
    Set-Content -LiteralPath $fake -Value '@echo lca 9.9.9' -Encoding Ascii
    $reported = Read-Version $fake
    Remove-Item -LiteralPath $fake -Force -ErrorAction SilentlyContinue
    Assert ($reported -eq '9.9.9') "Read-Version: expected '9.9.9', got '$reported'"
    Pass 'Read-Version strips the name clap prints, once'
  } else {
    Skip 'Read-Version normalization' 'cmd.exe is not available on this host'
  }

  #############################################################################
  # FR-INSTALL-9 / FR-INSTALL-3: one real registry round-trip, restoring the
  # runner's original value afterwards. Idempotent across two runs.
  #############################################################################
  Setup
  $saved = $null
  if ($script:HasRegistry) { $saved = Get-UserPathState }
  if (-not $script:HasRegistry) {
    Skip 'the live PATH append round-trip'
    Teardown
  } else {
  try {
    Invoke-Installer @{ BaseUrl = $script:BaseUrl; InstallDir = $script:InstallDir }
    Assert ($script:LastCode -eq 0) "install with PATH: exit $($script:LastCode)`n$($script:LastOutput)"
    Invoke-Installer @{ BaseUrl = $script:BaseUrl; InstallDir = $script:InstallDir }
    Assert ($script:LastCode -eq 0) "second install with PATH: exit $($script:LastCode)`n$($script:LastOutput)"
    $now = Get-UserPathState
    $entries = @()
    if ($now.Raw) { $entries = @($now.Raw -split ';' | Where-Object { $_ -ne '' }) }
    $ours = @($entries | Where-Object { $_.TrimEnd('\') -ieq $script:InstallDir.TrimEnd('\') })
    Assert ($ours.Count -eq 1) "PATH: the install dir appears $($ours.Count) times after two runs, expected 1"
    if ($saved.Raw) {
      foreach ($entry in ($saved.Raw -split ';' | Where-Object { $_ -ne '' })) {
        $still = @($entries | Where-Object { $_ -ieq $entry })
        Assert ($still.Count -eq 1) "PATH: a pre-existing entry was lost: $entry"
      }
    }
    if ($null -ne $saved.Kind) {
      Assert ($now.Kind -eq $saved.Kind) 'PATH: the value kind was not preserved'
    }
    Pass 'the PATH append is idempotent and preserves the registry value'
  } finally {
    # Restore the runner's original user Path, including the case where it
    # had no user Path value at all before this test created one.
    if ($null -eq $saved.Raw) {
      try { [Environment]::SetEnvironmentVariable('Path', $null, 'User') } catch { }
    } else {
      Set-UserPathState $saved
    }
    Teardown
  }
  }

  #############################################################################
  # FR-INSTALL-9: the documented one-liner shape runs unmodified - the
  # scriptblock invocation with a named parameter, in a child shell.
  #############################################################################
  Setup
  try {
    $file = $Installer -replace "'", "''"
    $base = $script:BaseUrl -replace "'", "''"
    $dir = $script:InstallDir -replace "'", "''"
    $cmd = "& ([scriptblock]::Create((Get-Content -LiteralPath '$file' -Raw))) -BaseUrl '$base' -InstallDir '$dir' -NoPath"
    $shell = $null
    if ($PSVersionTable.PSEdition -eq 'Desktop') {
      $shell = 'powershell'
    } elseif (Get-Command pwsh -ErrorAction SilentlyContinue) {
      $shell = 'pwsh'
    } else {
      $shell = (Get-Process -Id $PID).Path
    }
    $output = & $shell -NoProfile -Command $cmd 2>&1 | Out-String
    $code = $LASTEXITCODE
    Assert ($code -eq 0) "one-liner shape: exit $code`n$output"
    Assert (Test-Path -LiteralPath (Join-Path $script:InstallDir 'lca.exe')) 'one-liner shape: nothing installed'
    Pass 'the scriptblock one-liner shape with a named parameter works'
  } finally { Teardown }

  #############################################################################
  # ADR-0043 / U2: the unstable line. The flag picks download/unstable for the
  # binary and its artifacts.sha256 (the stable line's checksums are poisoned
  # in this fixture, so consulting them at all would fail this row), the mixed
  # flags are a usage error, and - where the fixture executable can run - the
  # report carries the hash version in both directions.
  #
  # Variable names in this row avoid the installer's parameter names
  # (Version/InstallDir/NoPath/Uninstall/Unstable/BaseUrl): dot-sourcing
  # binds them INTO this scope with their types, and assigning a string to
  # the SwitchParameter-typed `$Unstable` throws.
  #############################################################################
  Setup
  try {
    $unstableDir = Join-Path $script:BaseUrl 'download\unstable'
    New-Item -ItemType Directory -Force -Path $unstableDir | Out-Null
    $hashver = New-HashVersionExe
    if ($hashver -and -not (Test-Path -LiteralPath $hashver)) { $hashver = $null }
    foreach ($name in $script:AssetNames) {
      if ($hashver) {
        Copy-Item -LiteralPath $hashver -Destination (Join-Path $unstableDir $name) -Force
      } else {
        Copy-Item -LiteralPath (Get-FixtureExe) -Destination (Join-Path $unstableDir $name) -Force
        Add-Content -LiteralPath (Join-Path $unstableDir $name) -Encoding Ascii -Value "unstable-$name"
      }
    }
    Write-Checksums $unstableDir
    $bin = Join-Path $script:InstallDir 'lca.exe'

    # Baseline over the stable line: the report below needs an "old" side.
    Invoke-Installer @{ BaseUrl = $script:BaseUrl; InstallDir = $script:InstallDir; NoPath = $true }
    Assert ($script:LastCode -eq 0) "baseline stable install: exit $($script:LastCode)`n$($script:LastOutput)"
    Assert (Test-Path -LiteralPath $bin) 'baseline stable install: lca.exe missing'

    # Poison the stable line: wrong hashes for its own bytes. An installer
    # that consulted latest's checksums would refuse here.
    $poison = @()
    foreach ($name in $script:AssetNames) { $poison += ('{0}  {1}' -f ('0' * 64), $name) }
    Set-Content -LiteralPath (Join-Path $script:BaseUrl 'latest\download\artifacts.sha256') -Value $poison -Encoding Ascii

    Invoke-Installer @{ BaseUrl = $script:BaseUrl; InstallDir = $script:InstallDir; NoPath = $true; Unstable = $true }
    Assert ($script:LastCode -eq 0) "unstable install: exit $($script:LastCode)`n$($script:LastOutput)"

    # The installed bytes are a file from download/unstable (byte proof: it
    # does not depend on the fixture being executable on this host).
    $installedHash = (Get-FileHash -Algorithm SHA256 -LiteralPath $bin).Hash
    $fromUnstable = $false
    foreach ($name in $script:AssetNames) {
      $candidate = Join-Path $unstableDir $name
      if ((Get-FileHash -Algorithm SHA256 -LiteralPath $candidate).Hash -eq $installedHash) { $fromUnstable = $true; break }
    }
    Assert $fromUnstable 'unstable: the installed bytes are not from download/unstable'
    Pass 'the unstable flag installs from download/unstable with its own checksums'

    # The hash-version report needs the fixture to answer --version here;
    # a compiled PE cannot run on this host, so that path is a named skip.
    $newVer = Read-Version $bin
    if ($hashver -and $newVer -eq '0.5.2.b6573049') {
      Assert ($script:LastOutput -match '-> 0\.5\.2\.b[0-9a-f]{7}') "unstable: hash version missing from the report:`n$($script:LastOutput)"
      Pass 'the report carries the hash version as the new side'

      $good = @()
      foreach ($name in $script:AssetNames) {
        $hash = (Get-FileHash -Algorithm SHA256 -LiteralPath (Join-Path $script:BaseUrl ('latest\download\' + $name))).Hash.ToLowerInvariant()
        $good += ('{0}  {1}' -f $hash, $name)
      }
      Set-Content -LiteralPath (Join-Path $script:BaseUrl 'latest\download\artifacts.sha256') -Value $good -Encoding Ascii
      Invoke-Installer @{ BaseUrl = $script:BaseUrl; InstallDir = $script:InstallDir; NoPath = $true }
      Assert ($script:LastCode -eq 0) "stable round trip: exit $($script:LastCode)`n$($script:LastOutput)"
      Assert ($script:LastOutput -match 'lca 0\.5\.2\.b[0-9a-f]{7} -> ') "round trip: hash version not reported as old:`n$($script:LastOutput)"
      Pass 'the round trip back to stable reports the hash version as the old side'
    } else {
      $reason = if ($hashver) { 'the compiled fixture exe cannot run on this host' } else { 'Add-Type could not compile a fixture exe' }
      Skip 'the hash-version old -> new report' $reason
    }

    Invoke-Installer @{ BaseUrl = $script:BaseUrl; InstallDir = $script:InstallDir; NoPath = $true; Unstable = $true; Version = '9.9.9' }
    Assert ($script:LastCode -eq 2) "mixed flags: exit $($script:LastCode), expected 2"
    Assert ($script:LastOutput -match 'cannot be combined') 'mixed flags: no usage message'
    Pass 'unstable with a pinned version is a usage error'
  } finally { Teardown }

  } catch {
  Fail "unexpected error: $($_.Exception.Message)"
}

Write-Host "1..$($script:Passed)"
Write-Host "install.ps1: $($script:Passed) passed"
exit 0
