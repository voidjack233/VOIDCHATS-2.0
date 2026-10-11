param(
    [Parameter(Mandatory=$true)]
    [ValidateSet('machine','up','ready','migrate','api','diagnostic','test','integration','logs','stop','tunnel')]
    [string]$Action,
    [Parameter(ValueFromRemainingArguments=$true)]
    [string[]]$CommandArguments
)
$ErrorActionPreference = 'Stop'
. (Join-Path $PSScriptRoot 'dev-env.ps1')
$composePath = Join-Path $DevRoot 'infra\development\compose.yaml'
$postgresImage = 'docker.io/library/postgres:18.6-bookworm@sha256:afc7e2d441324c0388fa80c3d24f733b4194a4eb7f47dd8ee2b08eb1a24a647c'

function Invoke-DevCommand {
    param([string]$Executable, [string[]]$Arguments)
    # Compose providers can echo container environment arguments. Redact before output.
    $savedPreference = $ErrorActionPreference
    $ErrorActionPreference = 'Continue'
    & $Executable @Arguments 2>&1 | ForEach-Object {
        $item = $_
        $safeLine = [string]$item
        if ($env:PGPASSWORD) { $safeLine = $safeLine.Replace($env:PGPASSWORD, '<redacted>') }
        Write-Output $safeLine
    }
    $commandExit = $LASTEXITCODE
    $ErrorActionPreference = $savedPreference
    if ($commandExit -ne 0) { throw "$Executable failed (exit $commandExit)" }
}
function Get-GoExecutable {
    if ($env:VOID_GO_BINARY) { return $env:VOID_GO_BINARY }
    $installedGo = Get-Command go -ErrorAction SilentlyContinue
    if ($installedGo -and ((& $installedGo.Source version) -notmatch 'windows/386')) { return $installedGo.Source }
    $portableGo = Join-Path ([System.IO.Path]::GetTempPath()) 'codex-void-backend-go1.27.1\go\bin\go.exe'
    if (Test-Path -LiteralPath $portableGo) { return $portableGo }
    throw 'Install 64-bit Go or set VOID_GO_BINARY to its executable.'
}
function Get-CloudflaredExecutable {
    $installed = Get-Command cloudflared -ErrorAction SilentlyContinue
    if ($installed) { return $installed.Source }
    $localTool = Join-Path $env:LOCALAPPDATA 'VOIDDev\tools\cloudflared.exe'
    if (Test-Path -LiteralPath $localTool) { return $localTool }
    throw 'Install cloudflared and add it to PATH, or use its documented local tool directory.'
}
function Invoke-Compose {
    param([string[]]$ComposeArguments)
    # Providers may print shell-escaped passwords, which literal redaction cannot
    # safely cover. Suppress their output entirely instead of rendering secrets.
    $savedPreference = $ErrorActionPreference
    $ErrorActionPreference = 'Continue'
    $providerOutput = & podman compose --env-file $DevEnvPath --file $composePath @ComposeArguments 2>&1
    $providerExit = $LASTEXITCODE
    $ErrorActionPreference = $savedPreference
    if ($providerExit -ne 0) { throw "Compose provider failed (exit $providerExit). Check engine connectivity and use the safe logs command." }
    Write-Output 'PostgreSQL Compose command completed.'
}

switch ($Action) {
    'machine' {
        $machines = & podman machine list --format json | ConvertFrom-Json
        if (-not $machines) { Invoke-DevCommand 'podman' @('machine','init','--cpus','2','--memory','2048','--disk-size','20','void-dev') }
        $machines = & podman machine list --format json | ConvertFrom-Json
        $machine = $machines | Where-Object Name -eq 'void-dev' | Select-Object -First 1
        if (-not $machine) { throw 'An existing different machine was found. Start it explicitly; no replacement will be created.' }
        if (-not $machine.Running) { Invoke-DevCommand 'podman' @('machine','start','--update-connection','void-dev') }
        Invoke-DevCommand 'podman' @('ps','--format','{{.Names}} {{.Status}}')
    }
    'up' {
        if (-not $env:PGPASSWORD -or $env:PGPASSWORD -eq 'replace-with-a-locally-generated-password') { throw 'Set a real PGPASSWORD in ignored .env first.' }
        if ($env:PODMAN_COMPOSE_PROVIDER) { Invoke-Compose @('up','--detach') }
        else {
            & podman container exists void-dev-database
            if ($LASTEXITCODE -eq 0) { Invoke-DevCommand 'podman' @('start','void-dev-database') }
            else {
                # Environment-only password avoids placing credentials on the command line.
                $env:POSTGRES_PASSWORD = $env:PGPASSWORD
                $env:POSTGRES_USER = $env:PGUSER
                $env:POSTGRES_DB = $env:PGDATABASE
                Invoke-DevCommand 'podman' @('volume','create','void-dev-postgres-data')
                Invoke-DevCommand 'podman' @('run','--detach','--name','void-dev-database','--restart','unless-stopped','--publish',"127.0.0.1:$($env:PGPORT):5432",'--volume','void-dev-postgres-data:/var/lib/postgresql','--env','POSTGRES_PASSWORD','--env','POSTGRES_USER','--env','POSTGRES_DB','--env','POSTGRES_INITDB_ARGS=--auth-host=scram-sha-256','--health-cmd','pg_isready -U "$POSTGRES_USER" -d "$POSTGRES_DB"','--health-interval','5s','--health-timeout','3s','--health-retries','12',$postgresImage)
            }
        }
    }
    'ready' { Invoke-DevCommand 'podman' @('exec','void-dev-database','pg_isready','-U',$env:PGUSER,'-d',$env:PGDATABASE) }
    'logs' { Invoke-DevCommand 'podman' @('logs','--tail','100','void-dev-database') }
    'stop' { Invoke-DevCommand 'podman' @('stop','void-dev-database') }
    'test' { Invoke-DevCommand 'node' (@((Join-Path $PSScriptRoot 'test-backend.mjs')) + $CommandArguments) }
    'tunnel' {
        # Check identity before forwarding: the API and Metro are separate origins.
        Invoke-DevCommand 'node' @((Join-Path $PSScriptRoot 'check-diagnostic.mjs'))
        $cloudflared = Get-CloudflaredExecutable
        $savedPassword = $env:PGPASSWORD
        $savedPostgresPassword = $env:POSTGRES_PASSWORD
        try {
            Remove-Item Env:PGPASSWORD -ErrorAction SilentlyContinue
            Remove-Item Env:POSTGRES_PASSWORD -ErrorAction SilentlyContinue
            Invoke-DevCommand $cloudflared @('tunnel','--url','http://127.0.0.1:8091')
        } finally {
            $env:PGPASSWORD = $savedPassword
            $env:POSTGRES_PASSWORD = $savedPostgresPassword
        }
    }
    default {
        $go = Get-GoExecutable
        Push-Location (Join-Path $DevRoot 'backend')
        try {
            switch ($Action) {
                'migrate' { Invoke-DevCommand $go @('run','./cmd/migrate') }
                'integration' { $env:VOID_POSTGRES_TEST = '1'; Invoke-DevCommand $go @('test','./internal/account','./internal/database') }
                'api' { & $go run ./cmd/api; if ($LASTEXITCODE -ne 0) { throw 'API exited unsuccessfully' } }
                'diagnostic' {
                    $savedPassword = $env:PGPASSWORD
                    $savedPostgresPassword = $env:POSTGRES_PASSWORD
                    try {
                        Remove-Item Env:PGPASSWORD -ErrorAction SilentlyContinue
                        Remove-Item Env:POSTGRES_PASSWORD -ErrorAction SilentlyContinue
                        & $go run ./cmd/diagnostic
                        if ($LASTEXITCODE -ne 0) { throw 'Diagnostic exited unsuccessfully' }
                    } finally {
                        $env:PGPASSWORD = $savedPassword
                        $env:POSTGRES_PASSWORD = $savedPostgresPassword
                    }
                }
            }
        } finally { Pop-Location }
    }
}
