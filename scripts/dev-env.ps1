# Load literal backend settings. Never evaluate shell code or print credentials.
$DevRoot = Split-Path -Parent $PSScriptRoot
$DevEnvPath = Join-Path $DevRoot '.env'
$allowedSettings = @('PGPASSWORD','PGHOST','PGPORT','PGUSER','PGDATABASE','PGSSLMODE','PGSSLROOTCERT','VOID_ENV','VOID_HTTP_ADDR','VOID_HTTP_HOST','VOID_HTTP_PORT','VOID_DIAGNOSTIC_ADDR')
if (Test-Path -LiteralPath $DevEnvPath) {
    foreach ($line in [System.IO.File]::ReadAllLines($DevEnvPath)) {
        if ($line -notmatch '^\s*(?:export\s+)?([A-Za-z_][A-Za-z0-9_]*)\s*=\s*(.*)$') { continue }
        $settingName = $Matches[1]
        $settingValue = $Matches[2].Trim()
        if ($settingName -notin $allowedSettings) { continue }
        if ($settingValue.Length -ge 2 -and (($settingValue.StartsWith("'") -and $settingValue.EndsWith("'")) -or ($settingValue.StartsWith('"') -and $settingValue.EndsWith('"')))) {
            $settingValue = $settingValue.Substring(1, $settingValue.Length - 2)
        }
        if (-not [Environment]::GetEnvironmentVariable($settingName, 'Process')) {
            [Environment]::SetEnvironmentVariable($settingName, $settingValue, 'Process')
        }
    }
}
foreach ($entry in @{ PGHOST='127.0.0.1'; PGPORT='15432'; PGUSER='void_dev'; PGDATABASE='void_dev'; PGSSLMODE='disable'; VOID_ENV='development' }.GetEnumerator()) {
    if (-not [Environment]::GetEnvironmentVariable($entry.Key, 'Process')) {
        [Environment]::SetEnvironmentVariable($entry.Key, $entry.Value, 'Process')
    }
}
$openSSH = Join-Path $env:WINDIR 'System32\OpenSSH'
if ((Test-Path -LiteralPath $openSSH) -and -not (Get-Command ssh-keygen -ErrorAction SilentlyContinue)) {
    $env:PATH = $openSSH + ';' + $env:PATH
}
if (-not $env:PODMAN_COMPOSE_PROVIDER) {
    $provider = Get-Command podman-compose -ErrorAction SilentlyContinue
    if ($provider) { $env:PODMAN_COMPOSE_PROVIDER = $provider.Source }
    elseif (Test-Path -LiteralPath (Join-Path $env:APPDATA 'Python')) {
        $providerFile = Get-ChildItem -LiteralPath (Join-Path $env:APPDATA 'Python') -Filter podman-compose.exe -Recurse -File | Select-Object -First 1
        if ($providerFile) { $env:PODMAN_COMPOSE_PROVIDER = $providerFile.FullName }
    }
}
