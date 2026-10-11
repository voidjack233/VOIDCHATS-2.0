# Local development infrastructure

The Go account API stores public account data in PostgreSQL. Native OpenMLS remains on the client; the backend does not receive identity private keys. These are setup and verification procedures. Recorded results and remaining device/external checks are in [development-verification.md](development-verification.md).

```text
Android debug build -> trusted LAN API :8080 -> host-loopback PostgreSQL :15432
Quick Tunnel        -> loopback health-only diagnostic :8091
Named Tunnel + Access -> loopback account API :8080
```

The development Compose project is `void-development`, its database container is `void-dev-database`, and its persistent volume is `void-dev-postgres-data`. [compose.yaml](../infra/development/compose.yaml) pins PostgreSQL 18.6 by image digest and mounts the PostgreSQL 18 data parent at `/var/lib/postgresql`. PostgreSQL is published only on `127.0.0.1:15432`; an existing native server on port 5432 can stay running. Metro remains on port 8081.

## Inspect the host first

Use 64-bit Go 1.26 or newer, Node.js, and Podman. A Compose provider is optional because the helpers have a direct Podman fallback. Cloudflare Tunnel requires a separate `cloudflared` installation and, for a named tunnel, your own Cloudflare account and hostname.

From the repository root in PowerShell:

```powershell
Get-Command podman, ssh-keygen, go, node, cloudflared -ErrorAction SilentlyContinue
podman version
wsl --version
wsl --list --verbose
podman machine list
podman system connection list
podman ps -a --format '{{.Names}} {{.Status}} {{.Ports}}'
podman volume ls
Get-NetTCPConnection -State Listen -LocalPort 5432,15432,8080,8081,8091 -ErrorAction SilentlyContinue
```

After installing a tool, reopen the terminal or add its actual installation directory to this terminal's `PATH`. Podman's machine initialization needs Windows OpenSSH Client, including `ssh-keygen`; enable that Windows optional feature if missing. The PowerShell helper adds the existing `Windows\System32\OpenSSH` directory to its process path when needed. It does not install OpenSSH or replace other machines.

`podman compose` invokes an external provider. If needed, install `podman-compose` using `python -m pip install --user podman-compose` and make its Scripts directory available, or set `PODMAN_COMPOSE_PROVIDER` to the executable. The PowerShell helper also searches the current user's Python installation for that provider. See the [Podman Compose reference](https://docs.podman.io/en/latest/markdown/podman-compose.1.html). Without a provider, `up` uses the same pinned image, container name, volume, and loopback publication with `podman run`.

Inspect existing containers and volumes before making changes. Do not delete an unrelated machine, database, volume, or port reservation to make this setup fit. If `void-dev-database` already exists with unexpected settings, investigate its owner and configuration before starting it.

## Create local credentials once

The root `.env` is ignored by Git. Preserve an existing file and password. For a new file, the following PowerShell command generates a random password locally and writes UTF-8 without a BOM, which also works with the Bash helper:

```powershell
if (-not (Test-Path -LiteralPath '.env')) {
    $passwordBytes = New-Object byte[] 32
    $passwordGenerator = [Security.Cryptography.RandomNumberGenerator]::Create()
    try { $passwordGenerator.GetBytes($passwordBytes) }
    finally { $passwordGenerator.Dispose() }
    $developmentPassword = [Convert]::ToBase64String($passwordBytes)
    $environmentTemplate = [IO.File]::ReadAllText((Join-Path (Get-Location) '.env.example'))
    [IO.File]::WriteAllText(
        (Join-Path (Get-Location) '.env'),
        $environmentTemplate.Replace('replace-with-a-locally-generated-password', $developmentPassword),
        [Text.UTF8Encoding]::new($false))
}
```

On a Bash host, this Node command creates the file exclusively and prints no password:

```bash
node --input-type=module <<'NODE'
import {existsSync, readFileSync, writeFileSync} from 'node:fs';
import {randomBytes} from 'node:crypto';
if (!existsSync('.env')) {
  const template = readFileSync('.env.example', 'utf8');
  writeFileSync('.env', template.replace('replace-with-a-locally-generated-password', randomBytes(32).toString('base64')), {flag: 'wx', mode: 0o600});
}
NODE
```

If an existing file still contains the example placeholder, replace it locally before starting PostgreSQL. Do not print `.env`, render a resolved Compose configuration, or paste container environment inspection into reports. The helpers parse only approved settings as literal values; they do not execute the file. Nonempty process environment settings take precedence. Use `PGHOST=127.0.0.1`, `PGPORT=15432`, `PGUSER=void_dev`, `PGDATABASE=void_dev`, and `PGSSLMODE=disable` for this loopback development database.

PostgreSQL initialization uses the password only when creating the database in an empty volume. Editing `PGPASSWORD` later does not change an existing PostgreSQL role's password. Keep the configured password consistent with the preserved volume; do not reset the volume as a password-recovery shortcut.

These are server credentials. Do not load this file through a mobile dotenv plugin, prefix them with `EXPO_PUBLIC_`, or put them in React Native configuration. Cloudflare credentials also belong outside the repository and mobile bundle.

## Start PostgreSQL, migrate, and run the API

Run each command from the repository root. Keep the API command running in its own terminal. If the verification instances already own ports 8080 or 8091, stop those recorded instances using the path-checked procedure in [the verification report](development-verification.md#running-host-processes) before starting foreground helpers.

| Action | Windows PowerShell | Bash host |
| --- | --- | --- |
| Start/check engine | `.\scripts\dev.ps1 machine` | `bash scripts/dev.sh machine` |
| Start database | `.\scripts\dev.ps1 up` | `bash scripts/dev.sh up` |
| Check database readiness | `.\scripts\dev.ps1 ready` | `bash scripts/dev.sh ready` |
| Apply migrations | `.\scripts\dev.ps1 migrate` | `bash scripts/dev.sh migrate` |
| Run API in foreground | `.\scripts\dev.ps1 api` | `bash scripts/dev.sh api` |
| Exercise live API | `.\scripts\dev.ps1 test` | `bash scripts/dev.sh test` |
| Database logs | `.\scripts\dev.ps1 logs` | `bash scripts/dev.sh logs` |
| Stop database, retain data | `.\scripts\dev.ps1 stop` | `bash scripts/dev.sh stop` |

The Windows `machine` action creates `void-dev` only when no machine exists and starts it with an updated connection. If a different machine already exists, it asks you to select/start it explicitly instead of replacing it. On native Linux, the Bash `machine` action checks the existing engine; no VM is needed. On macOS, initialize/start your Podman machine first. A WSL shell must have a working Podman connection of its own; do not assume it shares the Windows CLI connection or host loopback address.

The PowerShell helper accepts `VOID_GO_BINARY` for an explicit Go executable. The helpers load the local environment and use database port 15432. Running `go run ./cmd/api` directly without those settings uses the Go configuration's port 5432 default, so load the intended environment first.

Migrations are embedded, versioned SQL with checksums. Each run uses a transaction and advisory lock; repeating a completed migration is safe. Do not edit an applied SQL file: add a new migration. API startup checks database connectivity and the expected schema before opening its listener. It fails rather than switching to memory storage when PostgreSQL or migrations are unavailable.

Check the API from a second terminal:

```powershell
Invoke-RestMethod 'http://127.0.0.1:8080/health'
Invoke-RestMethod 'http://127.0.0.1:8080/ready'
```

Or use `curl http://127.0.0.1:8080/health` and `curl http://127.0.0.1:8080/ready` on Bash. Health returns `{"status":"ok"}`. Readiness returns `{"status":"ready"}` with HTTP 200 when its bounded database probe succeeds, or HTTP 503 and `{"status":"not_ready"}` otherwise. Account endpoint details are in [backend/README.md](../backend/README.md).

## Windows WSL 3.0.1 cgroup workaround

A known WSL 3.0.1/Podman-machine combination fails with systemd cgroup setup. The [upstream issue](https://github.com/podman-container-tools/podman/issues/29749) and [merged machine-image fix](https://github.com/podman-container-tools/podman-machine-os/pull/319) use `cgroupfs` for that environment. Prefer a corrected machine image. For an affected existing `void-dev` machine, inspect its current container configuration first, then apply the same machine-local drop-in:

```powershell
$cgroupWorkaround = [Convert]::ToBase64String([Text.Encoding]::UTF8.GetBytes("[engine]`ncgroup_manager=`"cgroupfs`"`n"))
podman machine ssh --username root void-dev "mkdir -p /usr/share/containers/containers.conf.d && echo $cgroupWorkaround | base64 --decode > /usr/share/containers/containers.conf.d/999-podman-machine-wsl-cgroupfs.conf"
podman machine ssh void-dev systemctl --user restart podman.service
podman machine ssh void-dev podman info --format '{{.Host.CgroupManager}}'
```

This changes only that VM's Podman configuration. It disables rootless cgroup resource limits; it is a compatibility workaround, not a production tuning recommendation. It does not require editing WSL controller configuration or deleting a machine.

A container/pod created before the change can retain an unusable systemd cgroup parent. The earlier `void-dev-postgres` and `pod_void-dev` objects, if present, are preserved stopped/Created. The current `void-development` project uses `void-dev-database` with the same named volume. Never run two PostgreSQL containers against that volume simultaneously.

## Verify persistence and automated checks

The live helper creates synthetic public account fixtures, checks readiness, creation/retrieval, duplicate preservation, missing IDs, and private-field rejection. It retains its public fixture; use a temporary record outside the repository to compare the same account after restarts:

```powershell
$fixturePath = Join-Path ([IO.Path]::GetTempPath()) 'void-public-account-fixture.json'
.\scripts\dev.ps1 test --record $fixturePath
```

Stop the foreground API with Ctrl+C, run `.\scripts\dev.ps1 api` again, then in the other terminal run:

```powershell
.\scripts\dev.ps1 test --verify-record $fixturePath
podman restart --time 10 void-dev-database
.\scripts\dev.ps1 ready
.\scripts\dev.ps1 test --verify-record $fixturePath
```

The verification reads the recorded VOID-ID and compares every returned field, including the UTC timestamp. On Bash, use `bash scripts/dev.sh test --record /tmp/void-public-account-fixture.json` and the corresponding `--verify-record` command. If readiness is still starting, wait briefly and retry `ready` before checking the record.

For backend checks, run from `backend/` with the appropriate Go executable:

```bash
go fmt ./...
go vet ./...
go test ./...
go test -race ./...
```

The race detector needs a supported 64-bit target and C compiler. Real PostgreSQL integration tests are opt-in: from the repository root run `.\scripts\dev.ps1 integration` or `bash scripts/dev.sh integration`. They use isolated temporary schemas, cover migrations and concurrent repository behavior, and clean up only their own schemas. Plain unit tests skip these database tests unless `VOID_POSTGRES_TEST=1` is set.

Frontend checks are `npm run typecheck`, `npm run lint`, and `node --test src/backend/backend.test.mjs`. Native MLS checks remain documented in [native/openmls/README.md](../native/openmls/README.md).

## Connect a physical Android phone over a trusted LAN

Find the address on the actual Wi-Fi/Ethernet adapter. In PowerShell, inspect `Get-NetIPAddress -AddressFamily IPv4` and `Get-NetConnectionProfile`; on Linux use `ip -4 address`. Ignore loopback, virtual machine, VPN, and disconnected adapter addresses.

The default API listener is loopback-only. To accept a trusted LAN connection, run:

```powershell
$env:VOID_HTTP_ADDR = '0.0.0.0:8080'
.\scripts\dev.ps1 api
```

On Bash use `VOID_HTTP_ADDR=0.0.0.0:8080 bash scripts/dev.sh api`. Check `http://192.168.0.13:8080/health` from the host and then from a phone on the same network, replacing that example with the current adapter address. A successful host test does not prove the phone can pass the Windows firewall.

If Windows labels your own trusted home network **Public**, manually change that network to **Private** in Windows Settings after confirming it is trusted. The helpers do not change network profiles or firewall rules. If required, an administrator can add a rule scoped to the verified local address, subnet, port, and Private profile:

```powershell
New-NetFirewallRule -DisplayName 'VOID development API (Private LAN)' -Direction Inbound -Action Allow -Protocol TCP -LocalPort 8080 -LocalAddress 192.168.0.13 -RemoteAddress 192.168.0.0/24 -Profile Private
```

Replace both address and subnet with the actual network; do not apply this example blindly. Do not open PostgreSQL to the LAN or forward these ports on the router. Remove the development rule when no longer needed.

In the Android debug app, finish the startup acknowledgments, select **Preview VOID**, then open Profile → Settings. Enter the API origin in the development backend diagnostic, for example `http://192.168.0.13:8080`, and check health. The selector starts empty and stays in memory; `localhost` on a phone refers to the phone itself. The check performs only `GET /health` with an eight-second deadline. It does not initialize an account or change MLS identity state.

HTTP cleartext is permitted only by the Android debug manifest and its debug-only network security resource. The main/release manifest disables cleartext, release URL validation requires HTTPS, and the diagnostic UI is absent in release builds. No iOS production transport exception is added. See [the frontend backend guide](../src/backend/README.md) and [Android network security configuration](https://developer.android.com/privacy-and-security/security-config).

## Quick Tunnel: health-only external diagnostic

A Quick Tunnel is a temporary public diagnostic URL. It must forward the separate health-only process, never the account API. This origin binds numeric loopback, opens no database, and serves account/readiness paths as JSON 404. Port 8091 avoids both the API's 8080 and Metro's 8081.

Start the diagnostic in a foreground terminal:

```powershell
.\scripts\dev.ps1 diagnostic
```

In another terminal run the identity preflight and then the tunnel helper:

```powershell
node scripts/check-diagnostic.mjs
.\scripts\dev.ps1 tunnel
```

The helper repeats the preflight: it requires the distinctive `X-VOID-Diagnostic-Origin: health-only` header, expected health JSON, and absent account/readiness routes before forwarding the fixed `http://127.0.0.1:8091` origin. It removes database passwords from the diagnostic/tunnel child process environment. PowerShell finds `cloudflared` on PATH or at `%LOCALAPPDATA%\VOIDDev\tools\cloudflared.exe`. On Bash use `bash scripts/dev.sh diagnostic` and, in a second terminal, `bash scripts/dev.sh tunnel`.

After a manual launch prints its actual temporary URL, test only health:

```powershell
.\scripts\dev.ps1 test https://YOUR-ACTUAL-TEMPORARY-URL --health-only
```

Use the equivalent Bash helper command on a Bash host. The URL changes across runs; do not invent, hardcode, or commit it. This check establishes external reachability of the diagnostic process, not database readiness or account access. Stop both foreground commands with Ctrl+C. Cloudflare documents Quick Tunnels as development/testing tools and notes the existing `config.yaml` compatibility caveat; use a separate temporary configuration context if necessary. See [Quick Tunnel documentation](https://developers.cloudflare.com/tunnel/get-started/quick-tunnels/).

## Named tunnel: configure Access before publishing the API

Account API exposure requires a named tunnel and Cloudflare Access. Before creating its DNS route or launching a tunnel that reaches the API, configure a self-hosted Access application for the entire intended hostname, a default-deny policy, and an allow policy limited to the intended developer identities. Do not use an Everyone or Bypass policy. Keep the API bound to loopback while setting up this external path.

Cloudflare browser login and account authorization are manual steps; automation must stop at that boundary. Follow the [locally managed tunnel guide](https://developers.cloudflare.com/tunnel/features/locally-managed-tunnels/create-local-tunnel/). In PowerShell, resolve the executable and log in/create the tunnel manually:

```powershell
$cloudflaredCommand = Get-Command cloudflared -ErrorAction SilentlyContinue
$cloudflaredExecutable = if ($cloudflaredCommand) { $cloudflaredCommand.Source } else { Join-Path $env:LOCALAPPDATA 'VOIDDev\tools\cloudflared.exe' }
if (-not (Test-Path -LiteralPath $cloudflaredExecutable)) { throw 'Install cloudflared first.' }
& $cloudflaredExecutable tunnel login
& $cloudflaredExecutable tunnel create void-development
$tunnelConfig = Join-Path $env:USERPROFILE '.cloudflared\void-dev.yaml'
```

On a Bash host with `cloudflared` on PATH:

```bash
cloudflared tunnel login
cloudflared tunnel create void-development
```

Copy [cloudflared.example.yaml](../infra/development/cloudflared.example.yaml) to a private location outside Git: `$tunnelConfig` above on Windows, or `~/.cloudflared/void-dev.yaml` on a Bash host. Preserve an existing configuration rather than overwriting it. Replace its tunnel UUID, external credentials-file path, hostname, Access team name, and application audience. Keep the API service at `http://127.0.0.1:8080`, keep the fallback `http_status:404`, and retain:

```yaml
originRequest:
  access:
    required: true
    teamName: YOUR-ACCESS-TEAM
    audTag:
      - YOUR-ACCESS-APPLICATION-AUDIENCE
```

This requires origin-side validation of the Access JWT for the configured team and audience, in addition to the edge Access policy. See Cloudflare's [tunnel Access configuration](https://developers.cloudflare.com/api/resources/zero_trust/subresources/tunnels/subresources/cloudflared/). Certificates, credentials JSON, tokens, and the completed local configuration stay outside Git. Do not put Access service tokens or other server secrets in the mobile app.

Validate the actual private configuration locally. Only after Access and origin validation are configured should you create the DNS route and launch. In PowerShell, continue using the executable and private path above:

```powershell
& $cloudflaredExecutable tunnel --config $tunnelConfig ingress validate
& $cloudflaredExecutable tunnel route dns YOUR-TUNNEL-UUID YOUR-CONFIGURED-HOSTNAME
& $cloudflaredExecutable tunnel --config $tunnelConfig run YOUR-TUNNEL-UUID
```

On a Bash host:

```bash
cloudflared tunnel --config /absolute/private/path/void-dev.yaml ingress validate
cloudflared tunnel route dns YOUR-TUNNEL-UUID YOUR-CONFIGURED-HOSTNAME
cloudflared tunnel --config /absolute/private/path/void-dev.yaml run YOUR-TUNNEL-UUID
```

Paths and hostname above are placeholders. Verify that an unauthenticated external request is blocked or redirected to Access login, then sign in as an allowed developer and verify health. If an unauthenticated request returns API JSON directly, stop the tunnel and correct Access/origin settings. Offline configuration validation alone does not prove external authentication is enforced.

The current mobile diagnostic sends no credentials and does not implement an Access login flow. A protected named hostname may therefore require browser-based verification for now. Cloudflare Access is a development perimeter; account ownership proof, application authentication, abuse controls, and reviewed production deployment remain future work.

## Stop without deleting data

Stop the API, diagnostic, and tunnels with Ctrl+C. Use `.\scripts\dev.ps1 stop` or `bash scripts/dev.sh stop` for PostgreSQL. The named volume remains intact; `up` starts the same database again. Stop the Podman machine only after checking that it does not host other work.

Do not use `down --volumes`, volume pruning, or machine removal for ordinary shutdown. Record separately whether a check ran on the host, a physical phone, or through an authenticated external path. A host LAN check, a configuration validation, and a live external/device check establish different things; use the [verification report](development-verification.md) for the checks actually completed.
