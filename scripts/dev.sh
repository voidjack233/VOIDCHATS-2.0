#!/usr/bin/env bash
set -euo pipefail
dev_root="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
env_file="$dev_root/.env"
compose_file="$dev_root/infra/development/compose.yaml"
image='docker.io/library/postgres:18.6-bookworm@sha256:afc7e2d441324c0388fa80c3d24f733b4194a4eb7f47dd8ee2b08eb1a24a647c'

# Literal dotenv reader, never source/eval a file containing credentials.
if [[ -f "$env_file" ]]; then
  while IFS= read -r line || [[ -n "$line" ]]; do
    line="${line%$'\r'}"
    [[ "$line" =~ ^[[:space:]]*(export[[:space:]]+)?([A-Za-z_][A-Za-z0-9_]*)[[:space:]]*=[[:space:]]*(.*)$ ]] || continue
    setting="${BASH_REMATCH[2]}"; value="${BASH_REMATCH[3]}"
    case "$setting" in PGPASSWORD|PGHOST|PGPORT|PGUSER|PGDATABASE|PGSSLMODE|PGSSLROOTCERT|VOID_ENV|VOID_HTTP_ADDR|VOID_HTTP_HOST|VOID_HTTP_PORT|VOID_DIAGNOSTIC_ADDR) ;; *) continue ;; esac
    if [[ "$value" == \"*\" || "$value" == \'*\' ]]; then value="${value:1:${#value}-2}"; fi
    if [[ -z "${!setting:-}" ]]; then export "$setting=$value"; fi
  done < "$env_file"
fi
export PGHOST="${PGHOST:-127.0.0.1}" PGPORT="${PGPORT:-15432}" PGUSER="${PGUSER:-void_dev}" PGDATABASE="${PGDATABASE:-void_dev}" PGSSLMODE="${PGSSLMODE:-disable}" VOID_ENV="${VOID_ENV:-development}"

case "${1:-}" in
  machine)
    # Native Linux needs no VM; macOS may need `podman machine init/start` first.
    podman info >/dev/null
    podman ps --format '{{.Names}} {{.Status}}'
    ;;
  up)
    [[ -n "${PGPASSWORD:-}" && "$PGPASSWORD" != replace-with-a-locally-generated-password ]] || { echo 'Set a real PGPASSWORD in ignored .env.' >&2; exit 1; }
    if command -v podman-compose >/dev/null || [[ -n "${PODMAN_COMPOSE_PROVIDER:-}" ]]; then
      # Providers can echo shell-escaped credentials. Do not render their output.
      if ! podman compose --env-file "$env_file" --file "$compose_file" up --detach >/dev/null 2>&1; then
        echo 'Compose failed; check engine/provider connectivity and run safe logs.' >&2; exit 1
      fi
      echo 'PostgreSQL Compose command completed.'
    elif podman container exists void-dev-database; then
      podman start void-dev-database
    else
      export POSTGRES_PASSWORD="$PGPASSWORD" POSTGRES_USER="$PGUSER" POSTGRES_DB="$PGDATABASE"
      podman volume create void-dev-postgres-data
      podman run --detach --name void-dev-database --restart unless-stopped \
        --publish "127.0.0.1:$PGPORT:5432" --volume void-dev-postgres-data:/var/lib/postgresql \
        --env POSTGRES_PASSWORD --env POSTGRES_USER --env POSTGRES_DB \
        --env POSTGRES_INITDB_ARGS=--auth-host=scram-sha-256 \
        --health-cmd 'pg_isready -U "$POSTGRES_USER" -d "$POSTGRES_DB"' \
        --health-interval 5s --health-timeout 3s --health-retries 12 "$image"
    fi
    ;;
  ready) podman exec void-dev-database pg_isready -U "$PGUSER" -d "$PGDATABASE" ;;
  logs) podman logs --tail 100 void-dev-database ;;
  stop) podman stop void-dev-database ;;
  migrate) cd "$dev_root/backend"; go run ./cmd/migrate ;;
  api) cd "$dev_root/backend"; go run ./cmd/api ;;
  diagnostic) cd "$dev_root/backend"; env -u PGPASSWORD -u POSTGRES_PASSWORD go run ./cmd/diagnostic ;;
  integration) cd "$dev_root/backend"; VOID_POSTGRES_TEST=1 go test ./internal/account ./internal/database ;;
  test) shift; node "$dev_root/scripts/test-backend.mjs" "$@" ;;
  tunnel)
    node "$dev_root/scripts/check-diagnostic.mjs"
    exec env -u PGPASSWORD -u POSTGRES_PASSWORD cloudflared tunnel --url http://127.0.0.1:8091
    ;;
  *) echo 'Usage: bash scripts/dev.sh {machine|up|ready|migrate|api|diagnostic|integration|test|logs|stop|tunnel}' >&2; exit 2 ;;
esac
