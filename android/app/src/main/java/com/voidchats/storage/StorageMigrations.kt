package com.voidchats.storage

import android.content.Context
import java.security.MessageDigest
import net.zetetic.database.sqlcipher.SQLiteDatabase

/** Checks applied SQL and upgrades the encrypted database in one transaction. */
internal class StorageMigrations(private val context: Context) {
  private data class Migration(val version: Int, val name: String, val checksum: String, val sql: String)

  fun migrate(database: SQLiteDatabase): Int {
    val migrations = load()
    database.beginTransaction()
    try {
      val current = database.version
      if (current < 0 || current > migrations.last().version) fail()
      val tables = database.rawQuery("SELECT name FROM sqlite_master WHERE type='table' AND name NOT LIKE 'sqlite_%'", emptyArray()).use { cursor ->
        buildSet { while (cursor.moveToNext()) add(cursor.getString(0)) }
      }
      if (current == 0) {
        if (tables.isNotEmpty()) fail()
      } else {
        if ("storage_migrations" !in tables) fail()
        val history = database.rawQuery("SELECT version,name,checksum FROM storage_migrations ORDER BY version", emptyArray()).use { cursor ->
          buildList { while (cursor.moveToNext()) add(Triple(cursor.getInt(0), cursor.getString(1), cursor.getString(2))) }
        }
        if (history.size != current) fail()
        for ((index, applied) in history.withIndex()) {
          val expected = migrations[index]
          if (applied.first != expected.version || applied.second != expected.name || applied.third != expected.checksum) fail()
        }
      }
      for (migration in migrations.filter { it.version > current }) {
        execute(database, migration.sql)
        database.execSQL("INSERT INTO storage_migrations(version,name,checksum,applied_at) VALUES(?,?,?,?)",
          arrayOf(migration.version, migration.name, migration.checksum, System.currentTimeMillis()))
        database.version = migration.version
      }
      database.setTransactionSuccessful()
      return migrations.last().version
    } catch (_: Exception) {
      throw StorageFailure("DATABASE_ERROR")
    } finally {
      database.endTransaction()
    }
  }

  private fun load(): List<Migration> {
    try {
      val names = context.assets.list("storage")?.filter { it.endsWith(".sql") } ?: emptyList()
      val migrations = names.map { name ->
        val match = Regex("^([0-9]+)_[a-z0-9_]+\\.sql$").matchEntire(name) ?: fail()
        val version = match.groupValues[1].toIntOrNull() ?: fail()
        if (version < 1) fail()
        val bytes = context.assets.open("storage/$name").use { it.readBytes() }
        val sql = bytes.toString(Charsets.UTF_8)
        if (sql.isBlank() || sql.contains('\u0000') || sql.contains('\r')) fail()
        val checksum = MessageDigest.getInstance("SHA-256").digest(bytes).joinToString("") { "%02x".format(it.toInt() and 0xff) }
        Migration(version, name, checksum, sql)
      }.sortedBy { it.version }
      if (migrations.isEmpty() || migrations.withIndex().any { (index, migration) -> migration.version != index + 1 }) fail()
      return migrations
    } catch (_: Exception) {
      throw StorageFailure("DATABASE_ERROR")
    }
  }

  private fun execute(database: SQLiteDatabase, sql: String) {
    // Numbered assets are reviewed application SQL, not caller-supplied input.
    // Trigger bodies contain semicolons, so execute each complete END; block.
    val statement = StringBuilder()
    var trigger = false
    for (line in sql.lineSequence()) {
      val trimmed = line.trim()
      if (trimmed.isEmpty() || trimmed.startsWith("--")) continue
      if (statement.isEmpty()) trigger = trimmed.startsWith("CREATE TRIGGER", ignoreCase = true)
      statement.append(line).append('\n')
      if ((!trigger && trimmed.endsWith(';')) || (trigger && trimmed.equals("END;", ignoreCase = true))) {
        database.execSQL(statement.toString())
        statement.setLength(0)
        trigger = false
      }
    }
    if (statement.isNotEmpty()) fail()
  }

  private fun fail(): Nothing = throw StorageFailure("DATABASE_ERROR")
}
