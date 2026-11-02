#!/usr/bin/env python3
"""Test migration 003 with seeded legacy tables in a private PostgreSQL cluster.

Uses the existing isolated runner's guarded local-cluster helpers. Never reads
.env, connects to an existing database, or changes a running backend. Checks
row/catalog/privilege preservation and rollback after both an early destination
collision and a late index-name collision. The private cluster is always removed.
"""

import importlib.util
import json
import os
from pathlib import Path
import pwd
import secrets
import subprocess
import sys
import tempfile


REPO = Path(__file__).resolve().parents[1]
# Loading the shared helper must not leave generated files in the checkout.
sys.dont_write_bytecode = True
spec = importlib.util.spec_from_file_location(
    "isolated_messaging", REPO / "scripts" / "test-messaging-isolated.py")
isolated = importlib.util.module_from_spec(spec)
spec.loader.exec_module(isolated)

MIGRATION = "003_clear_table_names.sql"
RENAMES = {
    "void_sessions": "sessions",
    "void_key_packages": "key_packages",
    "void_requests": "message_requests",
    "void_conversations": "conversations",
    "void_blocks": "blocks",
    "void_messages": "messages",
    "void_event_counters": "sync_cursors",
    "void_events": "sync_events",
    "void_messaging_schema": "messaging_schema",
}


def execute(state, env, sql=None, *, migration=None, expected_error=None):
    command = [state["psql"], "-X", "--no-password", "--host", state["socket_dir"],
               "--port", str(state["port"]), "--username", state["admin_role"],
               "--dbname", state["database"], "-v", "ON_ERROR_STOP=1", "-A", "-t"]
    if migration:
        command += ["-f", str(REPO / "migrations" / migration)]
    result = subprocess.run(command, input=sql, text=True, capture_output=True,
                            cwd=REPO, env=env)
    if expected_error:
        if result.returncode == 0 or expected_error not in result.stderr:
            raise RuntimeError("Expected migration failure was not observed: " + expected_error)
    elif result.returncode:
        raise RuntimeError("Isolated migration verification failed: " + result.stderr.strip())
    return result.stdout.strip()


def snapshot(state, env, renamed):
    result = {}
    for old, new in {"accounts": "accounts", **RENAMES}.items():
        table = new if renamed else old
        # Names are fixed constants above. Everything else is collected by OID,
        # so preserved foreign-key targets and index/constraint identity matter.
        result[new] = json.loads(execute(state, env, f"""
            SELECT jsonb_build_object(
                'table', jsonb_build_object('oid', c.oid, 'row_type', c.reltype,
                    'owner', c.relowner, 'acl', c.relacl, 'kind', c.relkind,
                    'persistence', c.relpersistence, 'rls', c.relrowsecurity,
                    'force_rls', c.relforcerowsecurity),
                'columns', (SELECT jsonb_agg(to_jsonb(a) ORDER BY a.attnum)
                    FROM pg_catalog.pg_attribute a
                    WHERE a.attrelid=c.oid AND a.attnum>0),
                'constraints', (SELECT COALESCE(jsonb_agg(jsonb_build_object(
                    'name', k.conname, 'catalog', to_jsonb(k)-'conname') ORDER BY k.oid), '[]')
                    FROM pg_catalog.pg_constraint k WHERE k.conrelid=c.oid),
                'indexes', (SELECT COALESCE(jsonb_agg(jsonb_build_object(
                    'name', ic.relname, 'catalog', to_jsonb(ix),
                    'owner', ic.relowner, 'acl', ic.relacl) ORDER BY ic.oid), '[]')
                    FROM pg_catalog.pg_index ix
                    JOIN pg_catalog.pg_class ic ON ic.oid=ix.indexrelid
                    WHERE ix.indrelid=c.oid),
                'rows', (SELECT COALESCE(jsonb_agg(to_jsonb(t) ORDER BY to_jsonb(t)::text), '[]')
                    FROM public.{table} t))
            FROM pg_catalog.pg_class c WHERE c.oid='public.{table}'::regclass;
        """))
    return result


def normalized_names(snapshot_before):
    # Only display identifiers may change; every other collected field must be
    # identical, including table/row-type/constraint/index OIDs and raw row data.
    result = json.loads(json.dumps(snapshot_before))
    for old, new in RENAMES.items():
        for category in ("constraints", "indexes"):
            for item in result[new][category]:
                if item["name"].startswith(old + "_"):
                    item["name"] = new + item["name"][len(old):]
    return result


def require_equal(expected, actual, label):
    if expected != actual:
        changed = [name for name in expected if expected[name] != actual.get(name)]
        raise RuntimeError(label + ": " + ", ".join(changed))


def seed(state, env):
    execute(state, env, """
        INSERT INTO public.accounts(void_id,username,auth_public_key) VALUES
            (1,'synthetic-alice',decode(repeat('01',32),'hex')),
            (2,'synthetic-bob',decode(repeat('02',32),'hex'));
        INSERT INTO public.void_sessions VALUES
            (decode(repeat('03',32),'hex'),1,now(),now()+interval '30 minutes');
        INSERT INTO public.void_key_packages VALUES
            ('00000000-0000-4000-8000-000000000011',2,repeat('a',64),
                'AQIDBA==',now()+interval '1 day',1,
                '00000000-0000-4000-8000-000000000012',NULL),
            ('00000000-0000-4000-8000-000000000013',1,repeat('b',64),
                'BQYHCA==',now()+interval '1 day',NULL,NULL,NULL);
        INSERT INTO public.void_requests VALUES
            ('00000000-0000-4000-8000-000000000021',
             '00000000-0000-4000-8000-000000000031',1,2,
             '00000000-0000-4000-8000-000000000022',
             '00000000-0000-4000-8000-000000000011','accepted',1,2,
             decode('00ff01fe02fd03fc','hex'));
        UPDATE public.void_key_packages
            SET request_id='00000000-0000-4000-8000-000000000021'
            WHERE key_package_id='00000000-0000-4000-8000-000000000011';
        INSERT INTO public.void_conversations VALUES
            ('00000000-0000-4000-8000-000000000031',
             '00000000-0000-4000-8000-000000000021',1,2,1,
             decode('ff00fe01fd02fc03','hex'));
        INSERT INTO public.void_blocks VALUES(2,1,now());
        INSERT INTO public.void_messages VALUES
            ('00000000-0000-4000-8000-000000000041',
             '00000000-0000-4000-8000-000000000031',1,2,
             '00000000-0000-4000-8000-000000000042',1,8,
             decode('80ff00aabbccddee','hex'));
        INSERT INTO public.void_event_counters VALUES(1,2,0),(2,1,1);
        INSERT INTO public.void_events VALUES
            ('00000000-0000-4000-8000-000000000051',2,1,
             decode('ff00aabbcc','hex'),NULL),
            ('00000000-0000-4000-8000-000000000052',1,1,
             decode('00ffccddee','hex'),'applied');
    """)


def verify(state, env):
    seed(state, env)
    before = snapshot(state, env, False)
    if any(not data["rows"] for data in before.values()):
        raise RuntimeError("Every legacy table must contain data before the migration test")

    execute(state, env, "CREATE TABLE public.messages(destination_conflict boolean);")
    execute(state, env, migration=MIGRATION,
            expected_error="Destination relation public.messages already exists")
    require_equal(before, snapshot(state, env, False), "Early collision changed legacy schema/data")
    execute(state, env, "DROP TABLE public.messages;")
    print("PASS: destination collision rejects migration without changing legacy tables", flush=True)

    # This name collides only after all table renames and several constraint
    # renames have executed. It proves actual rollback, beyond preflight checks.
    execute(state, env, "CREATE TABLE public.message_requests_active_pair(index_conflict boolean);")
    execute(state, env, migration=MIGRATION,
            expected_error='relation "message_requests_active_pair" already exists')
    require_equal(before, snapshot(state, env, False), "Late collision did not roll back all renames")
    execute(state, env, "DROP TABLE public.message_requests_active_pair;")
    print("PASS: late index collision rolls back all table/constraint/index renames", flush=True)

    execute(state, env, migration=MIGRATION)
    after = snapshot(state, env, True)
    require_equal(normalized_names(before), after,
                  "Successful migration changed rows, catalog identities or privileges")
    for old in RENAMES:
        if execute(state, env, f"SELECT to_regclass('public.{old}') IS NULL;") != "t":
            raise RuntimeError("Legacy table remains: " + old)
    for new in RENAMES.values():
        for category in ("constraints", "indexes"):
            if any(item["name"].startswith("void_") for item in after[new][category]):
                raise RuntimeError("Legacy constraint/index name remains on " + new)
    if execute(state, env, "SELECT version FROM public.messaging_schema;") != "1":
        raise RuntimeError("Schema marker version changed")
    print("PASS: every row, table/row-type/constraint/index OID, owner, ACL, column and FK preserved", flush=True)
    print("PASS: expected table/constraint/index names; accounts and schema version unchanged", flush=True)

    # Exercise the actual restricted runtime role after the rename; its existing
    # grants must still cover every renamed table without granting them again.
    role = state["runtime_role"]
    statements = [f"BEGIN; SET LOCAL ROLE {role};"]
    statements += [f"SELECT count(*) FROM public.{name};" for name in RENAMES.values()]
    statements += ["SELECT void_id,username,auth_public_key,created_at FROM public.accounts LIMIT 0;"]
    statements += ["INSERT INTO public.sync_cursors VALUES(3,0,0);"]
    # Account 3 does not exist: the still-enforced FK must reject this write.
    statements += ["ROLLBACK;"]
    execute(state, env, "\n".join(statements),
            expected_error='violates foreign key constraint "sync_cursors_owner_void_id_fkey"')
    require_equal(after, snapshot(state, env, True), "Rejected runtime write changed data")
    print("PASS: existing restricted-role grants still work and renamed FKs enforce references", flush=True)

    execute(state, env, migration=MIGRATION,
            expected_error="Expected persistent table public.void_sessions is missing")
    require_equal(after, snapshot(state, env, True), "Reapplication changed migrated schema/data")
    print("PASS: reapplying migration fails without changing migrated schema/data", flush=True)


def main():
    env = isolated.clean_environment()
    if os.getuid() == 0:
        raise RuntimeError("Run the isolated cluster as an unprivileged local user")
    for executable in ("initdb", "pg_ctl", "psql"):
        if not (isolated.PG_BIN / executable).is_file():
            raise RuntimeError("PostgreSQL 18 binaries are required")
    root = Path(tempfile.mkdtemp(prefix=isolated.PREFIX, dir="/tmp"))
    root.chmod(0o700)
    socket = root / "socket"
    socket.mkdir(mode=0o700)
    suffix = secrets.token_hex(8)
    state = {"root_dir": str(root), "data_dir": str(root / "data"),
             "socket_dir": str(socket), "port": 20000 + secrets.randbelow(30000),
             "database": "void_test_" + suffix, "runtime_role": "void_test_app_" + suffix,
             "admin_role": pwd.getpwuid(os.getuid()).pw_name,
             "pg_ctl": str(isolated.PG_BIN / "pg_ctl"), "psql": str(isolated.PG_BIN / "psql")}
    try:
        isolated.run([str(isolated.PG_BIN / "initdb"), "-D", state["data_dir"],
                      "--auth-local=trust", "--auth-host=reject", "--encoding=UTF8",
                      "--locale=C", "--no-instructions"], env=env, label="Private PostgreSQL initialization")
        with (root / "data" / "postgresql.conf").open("a") as config:
            config.write("\nlisten_addresses = ''\n")
            config.write("unix_socket_directories = '" + str(socket).replace("'", "''") + "'\n")
            config.write("unix_socket_permissions = 0700\nport = " + str(state["port"]) + "\n")
        isolated.run([state["pg_ctl"], "-D", state["data_dir"], "-l", str(root / "postgres.log"),
                      "-w", "start"], env=env, label="Private PostgreSQL startup")
        role = state["runtime_role"]
        isolated.psql(state, env, f"CREATE ROLE {role} LOGIN NOSUPERUSER NOCREATEDB NOCREATEROLE "
                      f"NOINHERIT NOREPLICATION NOBYPASSRLS; CREATE DATABASE {state['database']} TEMPLATE template0;",
                      database="postgres")
        execute(state, env, f"""
            REVOKE ALL ON DATABASE {state['database']} FROM PUBLIC;
            GRANT CONNECT ON DATABASE {state['database']} TO {role};
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
        """)
        for migration in ("001_messaging.sql", "002_sessions.sql",
                          "001_messaging_privileges.sql", "002_sessions_privileges.sql"):
            isolated.psql(state, env, migration=migration, runtime_role=role)
        verify(state, env)
    finally:
        isolated.stop_cluster(state, env)


if __name__ == "__main__":
    try:
        main()
    except (RuntimeError, OSError, ValueError, KeyError) as error:
        print("Isolated table rename verification failed: " + str(error), file=sys.stderr)
        sys.exit(1)
