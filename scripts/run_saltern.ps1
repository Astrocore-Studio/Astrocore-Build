# Native Windows launcher; all generated maps, logs and captures stay in .local.
[CmdletBinding()]
param(
    [ValidateSet('Import', 'Server', 'Client', 'All', 'Smoke')]
    [string]$Mode = 'All',
    [string]$SourceRoot,
    [string]$MapPath,
    # Карта SS14 относительно Resources\Maps (saltern.yml, Test\dev_map.yml, ...); из её имени
    # берутся имя карты, файл импорта и имя журнала.
    [string]$Map = 'saltern.yml',
    [switch]$Autoplay,
    [switch]$NoBuild,
    [switch]$Reimport,
    [switch]$RefreshGeneratedMap,
    [ValidateSet('Auto', 'Vulkan', 'Dx12')]
    [string]$GraphicsBackend = 'Auto',
    [switch]$LocalEngine,
    [switch]$TraceTicks,
    [string]$EngineRoot,
    [ValidatePattern('^[a-zA-Z0-9_-]+$')]
    [string]$LogName,
    [Parameter(ValueFromRemainingArguments = $true)]
    [string[]]$ClientArguments
)
$ErrorActionPreference = 'Stop'
. "$PSScriptRoot\_dev_env.ps1"
Set-Location $AstroBuildRoot
if (-not $SourceRoot) {
    $SourceRoot = [IO.Path]::GetFullPath((Join-Path $AstroBuildRoot '..\..\space-station-14'))
}
$AstroMapStem = [IO.Path]::GetFileNameWithoutExtension($Map)
if (-not $LogName) { $LogName = $AstroMapStem -replace '[^a-zA-Z0-9_-]', '_' }
if (-not $MapPath) { $MapPath = Join-Path $AstroLocalRoot ('maps\' + $AstroMapStem + '.ron') }
$MapPath = [IO.Path]::GetFullPath($MapPath)
$AstroLogRoot = Join-Path $AstroLocalRoot 'logs'
New-Item -ItemType Directory -Force -Path (Split-Path -Parent $MapPath),$AstroLogRoot | Out-Null

function Invoke-AstroCargo {
    & cargo @args
    if ($LASTEXITCODE -ne 0) { throw "Cargo exited with code $LASTEXITCODE" }
}

if (-not $NoBuild) {
    if ($LocalEngine) {
        $AstroPrepareArgs = @('scripts/prepare_local_engine.py')
        if ($EngineRoot) { $AstroPrepareArgs += @('--engine', $EngineRoot) }
        & python @AstroPrepareArgs
        if ($LASTEXITCODE -ne 0) { throw 'Could not prepare the local Engine manifest.' }
        if ($Mode -eq 'Import') {
            Invoke-AstroCargo build --manifest-path .local/dev/Cargo.toml --bin import-map
        } else {
            Invoke-AstroCargo build --manifest-path .local/dev/Cargo.toml --bins
        }
    } elseif ($Mode -eq 'Import') {
        Invoke-AstroCargo build --locked -p tools --bin import-map
    } else {
        Invoke-AstroCargo build --locked -p tools --bin import-map -p server -p client
    }
}
$AstroDebugRoot = Join-Path $env:CARGO_TARGET_DIR 'debug'
$AstroMapSource = Join-Path $SourceRoot (Join-Path 'Resources\Maps' $Map)
if ($RefreshGeneratedMap -and (Test-Path -LiteralPath $MapPath)) {
    $mapTime = (Get-Item -LiteralPath $MapPath).LastWriteTimeUtc
    foreach ($inputFile in @($AstroMapSource, (Join-Path $AstroDebugRoot 'import-map.exe'))) {
        if ((Test-Path -LiteralPath $inputFile) -and (Get-Item -LiteralPath $inputFile).LastWriteTimeUtc -gt $mapTime) { $Reimport = $true }
    }
}
if ($Mode -ne 'Client' -and ($Mode -eq 'Import' -or $Reimport -or -not (Test-Path -LiteralPath $MapPath))) {
    # Должности станции берутся из прототипа карты (`gameMap`) в Resources\Prototypes\Maps.
    & (Join-Path $AstroDebugRoot 'import-map.exe') $AstroMapSource $MapPath $AstroMapStem --proto-root (Join-Path $SourceRoot 'Resources\Prototypes')
    if ($LASTEXITCODE -ne 0) { throw "Import of $Map failed: $LASTEXITCODE" }
}
if (-not (Test-Path -LiteralPath $MapPath)) { throw "Map not found: $MapPath" }
if ($Mode -eq 'Import') { return }
$env:ASTRO_MAP = $MapPath
$AstroAddress = if ($env:ASTRO_ADDR) { $env:ASTRO_ADDR } else { '127.0.0.1:7779' }
$AstroKeyFile = Join-Path $AstroBuildRoot '.astro-key'
if ($env:ASTRO_KEY) {
    $AstroKey = $env:ASTRO_KEY
} elseif (Test-Path -LiteralPath $AstroKeyFile) {
    $AstroKey = (Get-Content -LiteralPath $AstroKeyFile -Raw).Trim()
} else {
    $AstroKeyBytes = New-Object byte[] 32
    $AstroRng = [Security.Cryptography.RandomNumberGenerator]::Create()
    try { $AstroRng.GetBytes($AstroKeyBytes) } finally { $AstroRng.Dispose() }
    $AstroKey = -join ($AstroKeyBytes | ForEach-Object { $_.ToString('x2') })
    [IO.File]::WriteAllText($AstroKeyFile, $AstroKey)
}
if ($AstroKey -notmatch '^[0-9a-fA-F]{64}$') { throw 'ASTRO_KEY must contain 64 hexadecimal characters.' }
$AstroServerExe = Join-Path $AstroDebugRoot 'server.exe'
$AstroServerArgs = @('--listen', $AstroAddress, '--key', $AstroKey, '--map', $MapPath)
if ($TraceTicks) { $AstroServerArgs += '--trace-ticks' }
if ($Mode -eq 'Server') {
    & $AstroServerExe @AstroServerArgs
    exit $LASTEXITCODE
}
$AstroClientArgs = @('--connect-lightyear', $AstroAddress, '--ly-key', $AstroKey,
    '--world-id', 'polygon', '--render-manifest', (Join-Path $AstroBuildRoot 'assets\ss14\render-manifest.json'))
if ($Autoplay -or $Mode -eq 'Smoke') { $AstroClientArgs += '--autoplay' }
$AstroPreviousSmokeExit = $env:ASTRO_SMOKE_EXIT
$AstroPreviousShot = $env:ASTRO_SHOT
$AstroPreviousBackend = $env:WGPU_BACKEND
if ($GraphicsBackend -ne 'Auto') { $env:WGPU_BACKEND = $GraphicsBackend.ToLowerInvariant() }
if ($Mode -eq 'Smoke') {
    if (-not $env:ASTRO_SMOKE_EXIT) { $env:ASTRO_SMOKE_EXIT = '1' }
    $AstroCaptureRoot = Join-Path $AstroLocalRoot 'screenshots'
    New-Item -ItemType Directory -Force -Path $AstroCaptureRoot | Out-Null
    if (-not $env:ASTRO_SHOT) { $env:ASTRO_SHOT = $AstroCaptureRoot }
    New-Item -ItemType Directory -Force -Path $env:ASTRO_SHOT | Out-Null
    $AstroCaptureStarted = [DateTime]::UtcNow
    $AstroClientArgs += @('--screenshot', (Join-Path $AstroCaptureRoot ($AstroMapStem + '.png')), '--frames', '5000')
}
$AstroClientArgs += $ClientArguments
$AstroServerProcess = $null
try {
    if ($Mode -in @('All', 'Smoke')) {
        # Start-Process joins arguments; quote the map path to support spaces.
        $AstroSpawnArgs = @('--listen', $AstroAddress, '--key', $AstroKey, '--map', ('"' + $MapPath + '"'))
        if ($Mode -eq 'Smoke') { $AstroSpawnArgs += @('--lobby', '0') }
        if ($TraceTicks) { $AstroSpawnArgs += '--trace-ticks' }
        $AstroServerProcess = Start-Process -FilePath $AstroServerExe -ArgumentList $AstroSpawnArgs `
            -WorkingDirectory $AstroBuildRoot -WindowStyle Hidden -PassThru `
            -RedirectStandardOutput (Join-Path $AstroLogRoot "$LogName-server.stdout.log") `
            -RedirectStandardError (Join-Path $AstroLogRoot "$LogName-server.stderr.log")
        $AstroReadyDeadline = [DateTime]::UtcNow.AddMinutes(3)
        $AstroServerReady = $false
        while ([DateTime]::UtcNow -lt $AstroReadyDeadline) {
            if ($AstroServerProcess.HasExited) { throw "Server exited; see .local/logs/$LogName-server.stderr.log." }
            $AstroServerOutput = Get-Content -LiteralPath (Join-Path $AstroLogRoot "$LogName-server.stdout.log") -Raw -ErrorAction SilentlyContinue
            if ($AstroServerOutput -match '(?m)^Lightyear: ') { $AstroServerReady = $true; break }
            Start-Sleep -Milliseconds 100
        }
        if (-not $AstroServerReady) { throw "Server initialization timed out. See .local/logs/$LogName-server.stderr.log." }
        Write-Host "Saltern server PID: $($AstroServerProcess.Id)"
    }
    $AstroQuotedClientArgs = @($AstroClientArgs | Where-Object { $null -ne $_ } | ForEach-Object {
        '"' + $_.Replace('"', '\"') + '"'
    })
    $AstroClientProcess = Start-Process -FilePath (Join-Path $AstroDebugRoot 'client.exe') `
        -ArgumentList $AstroQuotedClientArgs -WorkingDirectory $AstroBuildRoot `
        -WindowStyle Normal -PassThru -Wait `
        -RedirectStandardOutput (Join-Path $AstroLogRoot "$LogName-client.stdout.log") `
        -RedirectStandardError (Join-Path $AstroLogRoot "$LogName-client.stderr.log")
    $AstroClientExitCode = $AstroClientProcess.ExitCode
    if ($null -ne $AstroClientExitCode -and $AstroClientExitCode -ne 0) {
        throw "Client exited with code $AstroClientExitCode; see .local/logs/$LogName-client.stderr.log."
    }
    if ($Mode -eq 'Smoke') {
        $AstroFailure = Select-String -LiteralPath (Join-Path $AstroLogRoot "$LogName-client.stderr.log") `
            -Pattern 'ConnectTokenExpired|ConnectionTimedOut|Invalid surface' -ErrorAction SilentlyContinue
        if ($AstroFailure) { throw "Client connection or renderer failed; see .local/logs/$LogName-client.stderr.log." }
        if ($env:ASTRO_AUTOPLAY_WIRES -and -not (Select-String -LiteralPath (Join-Path $AstroLogRoot "$LogName-client.stderr.log") -SimpleMatch 'ASTRO_WIRES: Some')) {
            throw "The wire interface did not open; see .local/logs/$LogName-client.stderr.log."
        }
        if ($env:ASTRO_AUTOPLAY_REPAIR -and -not (Select-String -LiteralPath (Join-Path $AstroLogRoot "$LogName-client.stderr.log") -SimpleMatch 'ASTRO_REPAIR_VENDOR: broken=false')) {
            throw "Repair did not restore the damaged vendor; see .local/logs/$LogName-client.stderr.log."
        }
        $AstroLastCapture = Get-Item -LiteralPath (Join-Path $env:ASTRO_SHOT 'frame340.png') -ErrorAction SilentlyContinue
        if (-not $AstroLastCapture -or $AstroLastCapture.LastWriteTimeUtc -lt $AstroCaptureStarted) {
            throw "The client exited without a capture of the loaded world; see .local/logs/$LogName-client.stderr.log."
        }
    }
} finally {
    $env:WGPU_BACKEND = $AstroPreviousBackend
    if ($Mode -eq 'Smoke') {
        $env:ASTRO_SMOKE_EXIT = $AstroPreviousSmokeExit
        $env:ASTRO_SHOT = $AstroPreviousShot
    }
    # Stop only the server started by this launcher.
    if ($AstroServerProcess -and -not $AstroServerProcess.HasExited) {
        Stop-Process -Id $AstroServerProcess.Id -ErrorAction SilentlyContinue
    }
}
exit $AstroClientExitCode
