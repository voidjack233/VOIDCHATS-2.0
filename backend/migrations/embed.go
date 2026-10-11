// Package migrations embeds versioned, forward-only database migrations.
// Keep applied migration files unchanged; add a new version for schema edits.
package migrations

import "embed"

// Files contains the SQL shipped with the backend, independent of its working
// directory. There is no automatic destructive rollback path.
//
//go:embed *.up.sql
var Files embed.FS
