#!/usr/bin/env python3
"""Verify account/messaging migrations in a private, disposable PostgreSQL 18 cluster.

No existing PostgreSQL service, credentials, environment file or database is used.
The cluster listens only on a Unix socket inside its private /tmp directory.
Run normally to provision, run all tests and race-enabled integration tests, and
remove the cluster. --keep-running retains a successful cluster and prints the
path of a mode-0600 state JSON for subsequent HTTP/process restart verification.
Stop retained clusters with: python3 scripts/test-messaging-isolated.py --stop STATE
"""

import argparse
import datetime
import json
import os
from pathlib import Path
import pwd
import secrets
import shutil
import signal
import subprocess
import sys
import tempfile
from urllib.parse import quote, urlencode


REPO = Path(__file__).resolve().parents[1]
PG_BIN = Path("/usr/lib/postgresql/18/bin")
PREFIX = "void-messaging-pg-"


def clean_environment():
    # libpq settings inherited from a development shell must never select a
    # different service, password file, database or connection target.
    return {key: value for key, value in os.environ.items()
            if not key.startswith("PG") and not key.startswith("VOID_TEST_")
            and key not in ("DATABASE_URL", "VOID_DATABASE_URL",
                            "VOID_RUN_POSTGRES_INTEGRATION", "VOID_RUN_MESSAGING_INTEGRATION",
                            "VOID_RUN_MESSAGING_BLACKBOX", "VOID_MESSAGING_BLACKBOX_STATE")}


def run(args, *, env, label, sql=None, secrets_to_redact=()):
    result = subprocess.run(args, input=sql, text=True, capture_output=True,
                            cwd=REPO, env=env)
    if result.returncode:
        diagnostics = result.stdout + result.stderr
        for value in secrets_to_redact:
            diagnostics = diagnostics.replace(value, "[private connection]")
        # initdb/pg_ctl/psql are invoked without production credentials and only
        # see this synthetic cluster. Captured failures can therefore be useful.
        if diagnostics:
            print(diagnostics, file=sys.stderr, end="" if diagnostics.endswith("\n") else "\n")
        raise RuntimeError(f"{label} failed (exit {result.returncode})")
    return result.stdout.strip()


def connection_url(state, user):
    return ("postgresql://" + quote(user, safe="") + "@/"
            + quote(state["database"], safe="") + "?"
            + urlencode({"host": state["socket_dir"], "port": state["port"], "sslmode": "disable"}))


def psql(state, env, sql=None, migration=None, runtime_role=None, database=None):
    args = [state["psql"], "-X", "--no-password", "--host", state["socket_dir"],
            "--port", str(state["port"]), "--username", state["admin_role"],
            "--dbname", database or state["database"], "-v", "ON_ERROR_STOP=1", "-A", "-t"]
    if runtime_role:
        args += ["-v", "runtime_role=" + runtime_role]
    if migration:
        args += ["-f", str(REPO / "migrations" / migration)]
    return run(args, env=env, label="Isolated PostgreSQL schema setup", sql=sql)


def stop_cluster(state, env):
    root = Path(state["root_dir"])
    # A state file cannot direct cleanup at another cluster or arbitrary path.
    if (root.parent != Path("/tmp") or not root.name.startswith(PREFIX)
            or root.is_symlink() or root.resolve() != root or root.stat().st_uid != os.getuid()
            or root.stat().st_mode & 0o077
            or Path(state["data_dir"]) != root / "data"
            or Path(state["socket_dir"]) != root / "socket"
            or state["pg_ctl"] != str(PG_BIN / "pg_ctl")):
        raise RuntimeError("Refusing cleanup: state is not an owned private test cluster")
    if (root / "data" / "postmaster.pid").exists():
        run([state["pg_ctl"], "-D", state["data_dir"], "-m", "fast", "-w", "stop"],
            env=env, label="Isolated PostgreSQL shutdown")
    shutil.rmtree(root)
    print("Stopped and removed private PostgreSQL test cluster", flush=True)


def go_tests(state, env, race):
    test_env = env.copy()
    test_env.update({"VOID_TEST_DATABASE_URL": state["app_url"],
                     "VOID_TEST_ADMIN_URL": state["admin_url"],
                     "VOID_RUN_POSTGRES_INTEGRATION": "1",
                     "VOID_RUN_MESSAGING_INTEGRATION": "1"})
    command = ["go", "test", "-count=1", "-json"]
    if race:
        command += ["-race", "./tests/integration"]
    else:
        command += ["./..."]
    print("Running " + " ".join(command) + " against private PostgreSQL 18", flush=True)
    # Stream safe JSON output so long builds do not hide progress. In particular,
    # require integration tests actually to execute rather than silently skip.
    completed = skipped = 0
    failed = False
    package_output = {}
    with subprocess.Popen(command, stdout=subprocess.PIPE, stderr=subprocess.STDOUT,
                          text=True, cwd=REPO, env=test_env) as process:
        for line in process.stdout:
            for value in (state["app_url"], state["admin_url"]):
                line = line.replace(value, "[private connection]")
            try:
                event = json.loads(line)
            except json.JSONDecodeError:
                print(line, end="", flush=True)
                continue
            integration = event.get("Package", "").endswith("/tests/integration")
            if integration and event.get("Test"):
                if event.get("Action") == "pass":
                    completed += 1
                if event.get("Action") == "skip":
                    skipped += 1
            if event.get("Action") == "fail":
                failed = True
                if not integration:
                    print("".join(package_output.get(event.get("Package"), [])[-120:]), end="", flush=True)
            if event.get("Action") == "output":
                output = event.get("Output", "")
                package_output.setdefault(event.get("Package"), []).append(output)
                if integration or output.startswith(("ok  ", "FAIL", "?   ")):
                    print(output, end="", flush=True)
        returncode = process.wait()
    if returncode or failed or skipped or not completed:
        raise RuntimeError(f"Go {'race ' if race else ''}tests failed or integration tests skipped "
                           f"(exit={returncode}, integration_passed={completed}, integration_skipped={skipped})")
    print(f"Verified {completed} PostgreSQL integration tests; none skipped", flush=True)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--keep-running", action="store_true",
                        help="retain a successful cluster for further isolated verification")
    parser.add_argument("--stop", metavar="STATE", help="stop/remove a retained private test cluster")
    args = parser.parse_args()
    env = clean_environment()
    if args.stop:
        state_path = Path(args.stop)
        if state_path.is_symlink() or state_path.stat().st_uid != os.getuid() or state_path.stat().st_mode & 0o077:
            raise RuntimeError("Refusing non-private state file")
        stop_cluster(json.loads(state_path.read_text()), env)
        return 0
    if os.getuid() == 0:
        raise RuntimeError("Run the isolated cluster as an unprivileged local user")
    for executable in ("initdb", "pg_ctl", "psql"):
        if not (PG_BIN / executable).is_file():
            raise RuntimeError("PostgreSQL 18 binaries are required under " + str(PG_BIN))
    root = Path(tempfile.mkdtemp(prefix=PREFIX, dir="/tmp"))
    root.chmod(0o700)
    socket = root / "socket"
    socket.mkdir(mode=0o700)
    suffix = secrets.token_hex(8)
    state = {"version": 1, "root_dir": str(root), "data_dir": str(root / "data"),
             "socket_dir": str(socket), "port": 20000 + secrets.randbelow(30000),
             "database": "void_test_" + suffix, "runtime_role": "void_test_app_" + suffix,
             "admin_role": pwd.getpwuid(os.getuid()).pw_name, "pg_ctl": str(PG_BIN / "pg_ctl"),
             "psql": str(PG_BIN / "psql"),
             "created_at": datetime.datetime.now(datetime.timezone.utc).isoformat()}
    state["app_url"] = connection_url(state, state["runtime_role"])
    state["admin_url"] = connection_url(state, state["admin_role"])
    marker = root / ".void-isolated-messaging-cluster"
    with os.fdopen(os.open(marker, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600), "w") as output:
        output.write(state["database"] + "\n")
    retained = False
    try:
        run([str(PG_BIN / "initdb"), "-D", state["data_dir"], "--auth-local=trust",
             "--auth-host=reject", "--encoding=UTF8", "--locale=C", "--no-instructions"],
            env=env, label="Private PostgreSQL initialization")
        # No TCP listener, and the only socket path is inaccessible to other users.
        with (root / "data" / "postgresql.conf").open("a") as config:
            config.write("\nlisten_addresses = ''\n")
            config.write("unix_socket_directories = '" + str(socket).replace("'", "''") + "'\n")
            config.write("unix_socket_permissions = 0700\n")
            config.write("port = " + str(state["port"]) + "\n")
        run([state["pg_ctl"], "-D", state["data_dir"], "-l", str(root / "postgres.log"), "-w", "start"],
            env=env, label="Private PostgreSQL startup")
        role, database = state["runtime_role"], state["database"]
        version = int(psql(state, env, "SHOW server_version_num;", database="postgres"))
        if version // 10000 != 18:
            raise RuntimeError("Private PostgreSQL must have major version 18")
        psql(state, env, f"CREATE ROLE {role} LOGIN NOSUPERUSER NOCREATEDB NOCREATEROLE NOINHERIT NOREPLICATION NOBYPASSRLS;\n"
             f'CREATE DATABASE "{database}" TEMPLATE template0;', database="postgres")
        # Keep this bootstrap identical to scripts/test-integration.py, including
        # the existing application's column-scoped account permissions.
        psql(state, env, f'''
            REVOKE ALL ON DATABASE "{database}" FROM PUBLIC;
            GRANT CONNECT ON DATABASE "{database}" TO {role};
            REVOKE CREATE ON SCHEMA public FROM PUBLIC;
            GRANT USAGE ON SCHEMA public TO {role};
            CREATE TABLE public.accounts (
                id UUID PRIMARY KEY DEFAULT uuidv7(),
                void_id BIGINT NOT NULL UNIQUE CHECK (void_id > 0),
                username VARCHAR(128) NOT NULL CHECK (
                    char_length(username) BETWEEN 1 AND 32 AND username = btrim(username)),
                auth_public_key BYTEA NOT NULL UNIQUE CHECK (octet_length(auth_public_key) = 32),
                created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
            );
            GRANT INSERT (void_id, username, auth_public_key),
                SELECT (void_id, username, auth_public_key, created_at) ON public.accounts TO {role};
        ''')
        for migration in ("001_messaging.sql", "002_sessions.sql",
                          "001_messaging_privileges.sql", "002_sessions_privileges.sql",
                          "003_clear_table_names.sql"):
            psql(state, env, migration=migration, runtime_role=role)
            print("Applied " + migration + " to private disposable database", flush=True)
        go_tests(state, env, race=False)
        go_tests(state, env, race=True)
        if args.keep_running:
            state["server_pid"] = int((root / "data" / "postmaster.pid").read_text().splitlines()[0])
            state_path = root / "state.json"
            with os.fdopen(os.open(state_path, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600), "w") as output:
                json.dump(state, output, indent=2)
                output.write("\n")
            retained = True
            print("All tests passed. Private retained cluster state: " + str(state_path), flush=True)
        return 0
    except BaseException:
        # Logs contain only this runner's synthetic test traffic. PostgreSQL's
        # concise error/detail lines identify SQL failures hidden by safe API
        # errors without exposing SQL parameter values or connection strings.
        log = root / "postgres.log"
        if log.is_file():
            errors = [line for line in log.read_text(errors="replace").splitlines()
                      if any(marker in line for marker in ("ERROR:", "DETAIL:", "HINT:"))]
            if errors:
                print("Private PostgreSQL diagnostics (synthetic test traffic only):", file=sys.stderr)
                print("\n".join(errors[-40:]), file=sys.stderr)
        raise
    finally:
        if not retained:
            stop_cluster(state, env)


if __name__ == "__main__":
    def interrupted(signum, frame):
        raise KeyboardInterrupt
    signal.signal(signal.SIGTERM, interrupted)
    try:
        sys.exit(main())
    except (RuntimeError, OSError, ValueError, KeyError) as error:
        print("Isolated integration verification failed: " + str(error), file=sys.stderr)
        sys.exit(1)
    except KeyboardInterrupt:
        print("Isolated integration verification interrupted", file=sys.stderr)
        sys.exit(130)
