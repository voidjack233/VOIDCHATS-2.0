# Development verification — 2026-10-12

The infrastructure and backend were executed on the Windows development host. This report describes this run; see [DEVELOPMENT.md](DEVELOPMENT.md) for repeatable setup and commands.

| Component | Verified state |
| --- | --- |
| Podman | CLI 6.0.2; rootless WSL machine `void-dev` running |
| WSL | Updated from 2.5.9 to 3.0.1; kernel 6.18.40.1 |
| PostgreSQL | Pinned official 18.6-bookworm image; `void-dev-database` healthy |
| Database socket | Host loopback `127.0.0.1:15432`; database/user `void_dev` |
| Persistent storage | Named volume `void-dev-postgres-data`, mounted at `/var/lib/postgresql` |
| Go API | Running on the host with explicit LAN listener `0.0.0.0:8080` |
| Localhost API | `http://127.0.0.1:8080`; full HTTP checks passed |
| LAN API | `http://192.168.0.13:8080`; full HTTP checks passed **from this host** |
| Diagnostic origin | `http://127.0.0.1:8091`; health-only identity and exclusion of API routes verified |
| Cloudflare | Executable installed; Quick Tunnel launch blocked by automatic approval review; no public URL created |

The user's existing ignored `.env` password was used without printing or replacing it. The pre-existing native PostgreSQL listener on port 5432 was left untouched. Database and tunnel credentials were not added to Git.

## Changed files

| Area | Files |
| --- | --- |
| Infrastructure | [compose.yaml](../infra/development/compose.yaml), [cloudflared.example.yaml](../infra/development/cloudflared.example.yaml), [.env.example](../.env.example), [.gitattributes](../.gitattributes) |
| Backend storage | [postgres_repository.go](../backend/internal/account/postgres_repository.go), [database](../backend/internal/database), [migrations](../backend/migrations), [migration CLI](../backend/cmd/migrate/main.go), `go.mod`/`go.sum`, associated unit/integration tests |
| Backend lifecycle and HTTP | [API main](../backend/cmd/api/main.go), [configuration](../backend/internal/config/config.go), [transport](../backend/internal/transport/http), [diagnostic process](../backend/cmd/diagnostic/main.go), account/VOID-ID validation and associated tests |
| Developer helpers | [dev.ps1](../scripts/dev.ps1), [dev-env.ps1](../scripts/dev-env.ps1), [dev.sh](../scripts/dev.sh), [test-backend.mjs](../scripts/test-backend.mjs), [check-diagnostic.mjs](../scripts/check-diagnostic.mjs) |
| Frontend | [src/backend](../src/backend), [SettingsScreen.tsx](../src/settings/SettingsScreen.tsx) |
| Android | [main manifest](../android/app/src/main/AndroidManifest.xml), [debug manifest](../android/app/src/debug/AndroidManifest.xml), [debug network security](../android/app/src/debug/res/xml/development_network_security_config.xml) |
| Documentation | [development guide](DEVELOPMENT.md), this report, root/backend READMEs, frontend backend README |

## Executed checks

- Go formatting, vet, full unit tests, and build passed with the verified amd64 Go 1.27.1 toolchain. The installed Windows Go executable is 32-bit; the development helper selects the available amd64 toolchain.
- All **nine PostgreSQL integration tests** passed against the running database: repository round trips, exact timestamps, concurrent duplicate detection, pool reopening, query deadlines, byte limits, migration concurrency, migration history, and schema drift. Each test used its own schema and cleaned only that schema.
- `go test -race ./...` passed in the official Linux Go 1.27.1-bookworm Podman builder. After the diagnostic port/header update, race tests for the affected diagnostic and configuration packages also passed. Builder containers were removed after completion; their source mount was read-only.
- Live localhost and host-LAN HTTP checks passed: health/readiness, creation, retrieval, duplicate 409 without overwrite, missing-account 404, and private-field 400 without storage. Fixtures contained synthetic public metadata only.
- The **same recorded account**, including its timestamp, survived an API process restart and `podman restart --time 10 void-dev-database`.
- Frontend TypeScript and lint passed; all seven backend-selector/health tests passed.
- Android debug and release manifest processing passed. Debug allows development HTTP; release disallows cleartext and omits the debug network-security resource. The backend diagnostic UI was absent from the release bundle.
- All seven existing native OpenMLS tests passed. The MLS implementation was unchanged.
- PowerShell parser, Bash syntax, Node syntax, diagnostic preflight, and helper argument forwarding checks passed.
- The HTTP verification helper rejected thirteen invalid or conflicting URL/option cases before any request, using a network-free mock. Invalid URL errors did not echo the synthetic credential marker; uppercase HTTP used the supplied origin.
- Named Cloudflare ingress configuration passed offline validation using synthetic UUID/hostname/Access settings: the configured hostname selects the API origin and other hostnames select the 404 rule. This does not verify live Access enforcement.

## Host compatibility repair

The original Podman machine image hit the WSL 6.6 nftables compatibility failure. Updating WSL resolved that failure, but WSL 3.0.1 exposed a cgroup delegation issue. The machine-local `cgroup_manager="cgroupfs"` workaround follows [the upstream Podman fix](https://github.com/podman-container-tools/podman-machine-os/pull/319). The original created pod/container retained its systemd parent; both were preserved in their stopped/Created states, and the new Compose project started `void-dev-database` against the same named volume. No existing machine, container, image, or volume was deleted.

Rootless cgroup resource limits are unavailable with this WSL workaround. This is development infrastructure; production resource isolation and least-privilege database roles need separate setup.

## Manual verification and boundaries

- No Android device was attached to ADB. Phone access and the on-device diagnostic remain unverified.
- Windows reports the Ethernet network as **Public**, with the firewall enabled. No network category or firewall rule was changed. The development guide explains how to mark a trusted LAN Private and permit only the API port from that subnet. The host-LAN check does not prove another device can reach the listener.
- Quick Tunnel was prepared exclusively for the identified health-only origin. Automatic approval review rejected its launch with “blocked by policy,” so external HTTPS verification remains manual.
- Named Cloudflare Tunnel requires the developer's account, chosen hostname, interactive authorization, and an Access policy. These were not supplied or created. Keep credentials outside Git and enforce Access JWT validation at the tunnel origin before routing the account API.
- The registration API still lacks authenticated identity ownership and abuse protection. Public application access requires an authentication/ownership design, authorization, rate limits, and appropriate TLS and operational controls. Cloudflare Access is development access control, not the native account authentication protocol.

## Running host processes

The API and diagnostic binaries were started in hidden background processes from `%LOCALAPPDATA%\VOIDDev\runtime`. Their logs, PID metadata (`processes.json`), and public persistence fixture are outside Git. Stop these verification instances before starting the foreground helpers on the same ports. Use the following in PowerShell; it checks executable paths before stopping the recorded PIDs:

```powershell
$runtimeDir = Join-Path $env:LOCALAPPDATA 'VOIDDev\runtime'
$state = Get-Content (Join-Path $runtimeDir 'processes.json') -Raw | ConvertFrom-Json
foreach ($entry in @(@{ Id=$state.apiPID; File='void-api.exe' }, @{ Id=$state.diagnosticPID; File='void-diagnostic.exe' })) {
    $process = Get-Process -Id $entry.Id -ErrorAction SilentlyContinue
    if ($process -and $process.Path -eq (Join-Path $runtimeDir $entry.File)) {
        Stop-Process -Id $process.Id
    }
}
& scripts/dev.ps1 stop
```

This stops services without deleting their database volume. Foreground API, diagnostic, and tunnel helper processes can be stopped with Ctrl+C. No commit, push, merge, or production deployment was performed.
