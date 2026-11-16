#!/usr/bin/env python3
"""Run Go tests against a newly created PostgreSQL 18 database, then remove it.

Requires a local administrative PG* environment or --admin-env literal env file.
No credentials are printed; development accounts are never read or modified.
"""
import argparse
import getpass
import os
import re
from pathlib import Path
import secrets
import subprocess
import sys
from urllib.parse import quote, unquote, urlencode, urlsplit, urlunsplit, parse_qs


def read_env(path):
    values = {}
    if path:
        for raw in Path(path).read_text().splitlines():
            line = raw.strip()
            if not line or line.startswith("#"):
                continue
            key, sep, value = line.removeprefix("export ").partition("=")
            if not sep:
                raise RuntimeError("Invalid admin environment file")
            value = value.strip()
            if value[:1] in ("'", '"'):
                if len(value) < 2 or value[-1] != value[0]:
                    raise RuntimeError("Invalid admin environment quoting")
                value = value[1:-1]
            values[key.strip()] = value
    return values


def psql(env, sql):
    result = subprocess.run(
        ["psql", "-X", "--no-password", "-v", "ON_ERROR_STOP=1", "-A", "-t", "-v", "VERBOSITY=sqlstate"],
        input=sql, text=True, capture_output=True, env=env,
    )
    if result.returncode:
        # Do not print SQL/passwords from administrative diagnostics.
        state = re.search(r"(?:ERROR|FATAL):\s+([0-9A-Z]{5})\b", result.stderr)
        raise RuntimeError("PostgreSQL test setup/cleanup failed; SQLSTATE=" + (state.group(1) if state else "unavailable"))
    return result.stdout.strip()


def connection_url(env, database, user=None, password=None):
    if env.get("DATABASE_URL") and user is None:
        parts = urlsplit(env["DATABASE_URL"])
        if parts.scheme not in ("postgres", "postgresql"):
            raise RuntimeError("Expected a PostgreSQL connection URL")
        return urlunsplit((parts.scheme, parts.netloc, "/" + quote(database, safe=""), parts.query, ""))
    host = env.get("PGHOST", "localhost")
    if ":" in host and not host.startswith("["):
        host = "[" + host + "]"
    if host.startswith("/"):
        raise RuntimeError("Test runner requires a TCP PGHOST (e.g. localhost)")
    user = user if user is not None else env.get("PGUSER", getpass.getuser())
    password = password if password is not None else env.get("PGPASSWORD", "")
    return ("postgresql://" + quote(user, safe="") + ":" + quote(password, safe="")
            + "@" + host + ":" + env.get("PGPORT", "5432") + "/" + quote(database, safe="")
            + "?" + urlencode({"sslmode": env.get("PGSSLMODE", "prefer")}))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--allow-database-admin", action="store_true",
                        help="explicit opt-in to creating/dropping disposable PostgreSQL resources")
    parser.add_argument("--existing-login", action="store_true", help="use the configured login for disposable fixtures; create or change no roles")
    parser.add_argument("--admin-env", help="literal admin PG* env file; never evaluated as shell")
    parser.add_argument("command", nargs=argparse.REMAINDER, help="command after -- (default: go test ./...)")
    args = parser.parse_args()
    if not args.allow_database_admin:
        parser.error("Database administration is disabled. Explicit permission is required before using --allow-database-admin. No database connection or change was made.")
    command = args.command
    if command[:1] == ["--"]:
        command = command[1:]
    command = command or ["go", "test", "./..."]
    admin = read_env(args.admin_env)
    admin.update(os.environ)
    if admin.get("DATABASE_URL"):
        parts = urlsplit(admin["DATABASE_URL"])
        admin.update(PGHOST=parts.hostname or "localhost", PGPORT=str(parts.port or 5432),
                     PGUSER=unquote(parts.username or ""), PGPASSWORD=unquote(parts.password or ""),
                     PGDATABASE=unquote(parts.path.lstrip("/")))
        params = parse_qs(parts.query)
        if "sslmode" in params:
            admin["PGSSLMODE"] = params["sslmode"][0]
    admin.setdefault("PGDATABASE", "postgres")
    admin["PGCONNECT_TIMEOUT"] = "5"
    version = int(psql(admin, "SHOW server_version_num;"))
    if version // 10000 != 18:
        raise RuntimeError("Integration verification requires PostgreSQL 18")
    suffix = secrets.token_hex(8)
    database, role, password = "void_test_" + suffix, "void_test_app_" + suffix, secrets.token_hex(32)
    role_created = database_created = False
    try:
        if not args.existing_login:
            psql(admin, f"CREATE ROLE {role} LOGIN NOSUPERUSER NOCREATEDB NOCREATEROLE NOINHERIT NOREPLICATION NOBYPASSRLS PASSWORD '{password}';")
            role_created = True
        else:
            role = psql(admin, "SELECT current_user;")
        role_sql = '"' + role.replace('"', '""') + '"'
        psql(admin, f'CREATE DATABASE "{database}" TEMPLATE template0;')
        database_created = True
        test_admin = admin.copy()
        test_admin["PGDATABASE"] = database
        psql(test_admin, f'''
            REVOKE ALL ON DATABASE "{database}" FROM PUBLIC;
            GRANT CONNECT ON DATABASE "{database}" TO {role_sql};
            REVOKE CREATE ON SCHEMA public FROM PUBLIC;
            GRANT USAGE ON SCHEMA public TO {role_sql};
            CREATE TABLE public.accounts (
                id UUID PRIMARY KEY DEFAULT uuidv7(),
                void_id BIGINT NOT NULL UNIQUE CHECK (void_id > 0),
                auth_public_key BYTEA NOT NULL UNIQUE CHECK (octet_length(auth_public_key) = 32),
                created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
            );
            GRANT INSERT (void_id, auth_public_key),
                SELECT (void_id, auth_public_key, created_at) ON public.accounts TO {role_sql};
        ''')
        # Only this newly created disposable database receives test schema.
        # Exercise the exact reviewed prekey migration without touching .env/live DB.
        psql(test_admin, """
            CREATE TABLE public.messages (
                event_id uuid NOT NULL,
                sender_void_id bigint NOT NULL REFERENCES public.accounts(void_id),
                recipient_void_id bigint NOT NULL REFERENCES public.accounts(void_id),
                payload bytea NOT NULL,
                created_at timestamptz NOT NULL,
                expires_at timestamptz NOT NULL,
                PRIMARY KEY(sender_void_id,event_id),
                CONSTRAINT messages_payload_check CHECK(octet_length(payload) BETWEEN 1 AND 1048576),
                CONSTRAINT messages_expiry_check CHECK(expires_at=created_at+INTERVAL '720 hours')
            );
            CREATE INDEX messages_recipient_delivery_idx
                ON public.messages(recipient_void_id,created_at,sender_void_id,event_id);
            CREATE INDEX messages_expiry_idx ON public.messages(expires_at,sender_void_id,event_id);
        """)
        migration_dir = Path(__file__).resolve().parents[1] / "migrations"
        psql(test_admin, (migration_dir / "006_signal_public_prekeys.sql").read_text(encoding="utf-8"))
        grants = (migration_dir / "006_signal_public_prekeys_privileges.sql").read_text(encoding="utf-8")
        # The generated or existing login is quoted as a SQL identifier.
        psql(test_admin, grants.replace(':"runtime_role"', role_sql))
        psql(test_admin, (migration_dir / "007_account_deletion.sql").read_text(encoding="utf-8"))
        grants = (migration_dir / "007_account_deletion_privileges.sql").read_text(encoding="utf-8")
        psql(test_admin, grants.replace(':"runtime_role"', role_sql))
        psql(test_admin, f"GRANT SELECT,INSERT,DELETE ON public.messages TO {role_sql};")
        test_env = os.environ.copy()
        test_env["VOID_TEST_DATABASE_URL"] = connection_url(admin, database) if args.existing_login else connection_url(admin, database, role, password)
        if args.existing_login:
            test_env["VOID_TEST_OWNER_LOGIN"] = "1"
        test_env["VOID_TEST_ADMIN_URL"] = connection_url(admin, database)
        test_env["VOID_RUN_POSTGRES_INTEGRATION"] = "1"
        print(f"Running tests on disposable PostgreSQL {version // 10000} database {database}", flush=True)
        return subprocess.run(command, env=test_env).returncode
    finally:
        # These identifiers are generated here, never accepted from callers.
        if database_created:
            psql(admin, f'DROP DATABASE "{database}" WITH (FORCE);')
        if role_created:
            psql(admin, f"DROP ROLE {role};")
        print("Removed disposable PostgreSQL test resources", flush=True)


if __name__ == "__main__":
    try:
        sys.exit(main())
    except RuntimeError as error:
        print(str(error), file=sys.stderr)
        sys.exit(1)
    except (OSError, ValueError):
        print("Integration runner failed; check executable availability and configuration.", file=sys.stderr)
        sys.exit(1)
