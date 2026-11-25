// SPDX-License-Identifier: GPL-3.0-or-later
#include "resultstore.h"
#include <QDir>
#include <QJsonDocument>
#include <QFileInfo>
#include <sqlite3.h>

namespace hn {
namespace {
QString identity(const QJsonObject &event)
{
    const auto bytes = event.value("pathBytes").toString();
    return bytes.isEmpty() ? QStringLiteral("p:") + event.value("path").toString() : QStringLiteral("b:") + bytes;
}

QByteArray orderColumn(ResultOrder order)
{
    if (order == ResultOrder::Names) return "by_name";
    if (order == ResultOrder::Paths) return "by_path";
    return "by_tree";
}

QByteArray blob(sqlite3_stmt *s, int i)
{
    return QByteArray(static_cast<const char *>(sqlite3_column_blob(s, i)), sqlite3_column_bytes(s, i));
}

QString string(sqlite3_stmt *s, int i) { return QString::fromUtf8(blob(s, i)); }
void bindBytes(sqlite3_stmt *s, int index, const QByteArray &v)
{
    if (sqlite3_bind_blob64(s, index, v.constData(), sqlite3_uint64(v.size()), SQLITE_TRANSIENT) != SQLITE_OK)
        throw StoreError(sqlite3_errmsg(sqlite3_db_handle(s)));
}

void bindText(sqlite3_stmt *s, int index, const QString &v)
{
    const auto bytes = v.toUtf8();
    if (sqlite3_bind_text64(s, index, bytes.constData(), sqlite3_uint64(bytes.size()), SQLITE_TRANSIENT, SQLITE_UTF8) != SQLITE_OK)
        throw StoreError(sqlite3_errmsg(sqlite3_db_handle(s)));
}

QList<QByteArray> directoryKeys(const QJsonObject &match, const QStringList &parts)
{
    const auto raw = QByteArray::fromBase64(match.value("pathBytes").toString().toLatin1());
    QList<QByteArray> components;
    if (!raw.isEmpty() && match.value("pathEncoding") == "unix") {
        components = raw.split('/');
    } else if (!raw.isEmpty() && match.value("pathEncoding") == "utf16le") {
        int start = 0;
        for (int i = 0; i + 1 < raw.size(); i += 2) {
            if ((raw[i] == '/' || raw[i] == '\\') && raw[i + 1] == '\0') {
                components.append(raw.mid(start, i - start));
                start = i + 2;
            }
        }
        components.append(raw.mid(start));
    }
    QList<QByteArray> result;
    const auto skip = components.size() - parts.size();
    for (qsizetype n = 0; n + 1 < parts.size(); ++n)
        result.append((skip >= 0 ? components[skip + n] : parts[n].toUtf8()).toHex());
    return result;
}

QJsonObject object(sqlite3_stmt *s) { return QJsonDocument::fromJson(blob(s, 0)).object(); }
}

ResultStore::ResultStore()
{
    // The empty filename asks SQLite to create and unlink a temporary database.
    const int rc = sqlite3_open("", &database);
    try {
        checked(rc);
        execute("PRAGMA temp_store=FILE; PRAGMA cache_size=-2048; PRAGMA synchronous=OFF;"
                "CREATE TABLE files(id INTEGER PRIMARY KEY, identity TEXT NOT NULL UNIQUE, path TEXT NOT NULL,"
                " absolute TEXT NOT NULL, dirs BLOB NOT NULL, by_name BLOB NOT NULL, by_path BLOB NOT NULL,"
                " by_tree BLOB NOT NULL, hits INTEGER NOT NULL DEFAULT 0, names INTEGER NOT NULL DEFAULT 0,"
                " epoch INTEGER NOT NULL DEFAULT 0, prefix INTEGER NOT NULL DEFAULT 0, active INTEGER NOT NULL DEFAULT 0);"
                "CREATE UNIQUE INDEX names ON files(by_name) WHERE active=1; CREATE UNIQUE INDEX paths ON files(by_path) WHERE active=1;"
                "CREATE UNIQUE INDEX tree ON files(by_tree) WHERE active=1;"
                "CREATE TABLE matches(file INTEGER NOT NULL, epoch INTEGER NOT NULL, ordinal INTEGER NOT NULL,"
                " name_ordinal INTEGER, data BLOB NOT NULL, PRIMARY KEY(file,epoch,ordinal)) WITHOUT ROWID;"
                "CREATE UNIQUE INDEX filename_lookup ON matches(file,name_ordinal) WHERE name_ordinal IS NOT NULL;"
                "CREATE TABLE errors(id INTEGER PRIMARY KEY, data BLOB NOT NULL); BEGIN;");
    } catch (...) {
        sqlite3_close(database);
        throw;
    }
}

ResultStore::~ResultStore()
{
    for (auto *s : statements) sqlite3_finalize(s);
    sqlite3_close(database);
}

void ResultStore::checked(int rc) const
{
    if (rc != SQLITE_OK) throw StoreError(database ? sqlite3_errmsg(database) : sqlite3_errstr(rc));
}

void ResultStore::execute(const char *sql) const { checked(sqlite3_exec(database, sql, nullptr, nullptr, nullptr)); }
sqlite3_stmt *ResultStore::query(const QByteArray &sql) const
{
    auto *s = statements.value(sql);
    if (!s) {
        checked(sqlite3_prepare_v2(database, sql.constData(), -1, &s, nullptr));
        statements.insert(sql, s);
    }
    sqlite3_reset(s);
    sqlite3_clear_bindings(s);
    return s;
}

bool ResultStore::step(sqlite3_stmt *s) const
{
    const auto rc = sqlite3_step(s);
    if (rc == SQLITE_ROW) return true;
    if (rc != SQLITE_DONE) checked(rc);
    return false;
}

void ResultStore::flush()
{
    if (!changes) return;
    execute("COMMIT; BEGIN;");
    changes = 0;
}

void ResultStore::advanceBatch()
{
    if (++changes >= 512) flush();
}

qint64 ResultStore::ensureFile(const QJsonObject &match)
{
    const auto key = identity(match);
    if (lastFile && key == lastIdentity) return lastFile;
    auto *s = query("SELECT id FROM files WHERE identity=?");
    bindText(s, 1, key);
    if (step(s)) {
        lastFile = sqlite3_column_int64(s, 0);
        sqlite3_reset(s);
    } else {
        const auto absolute = match.value("path").toString();
        const auto path = QDir::fromNativeSeparators(match.value("displayPath").toString(QFileInfo(absolute).fileName()));
        const auto parts = path.split('/');
        const auto dirs = directoryKeys(match, parts);
        // NUL terminates a component. Native path components cannot contain it.
        const auto suffix = path.toUtf8() + '\0' + key.toUtf8();
        QByteArray tree;
        for (qsizetype i = 0; i < dirs.size(); ++i)
            tree += '\1' + parts[i].toUtf8() + '\0' + dirs[i] + '\0';
        tree += '\2' + parts.last().toUtf8() + '\0' + key.toUtf8();
        s = query("INSERT INTO files(identity,path,absolute,dirs,by_name,by_path,by_tree) VALUES(?,?,?,?,?,?,?)");
        bindText(s, 1, key);
        bindText(s, 2, path);
        bindText(s, 3, absolute);
        bindBytes(s, 4, dirs.join('/'));
        bindBytes(s, 5, parts.last().toUtf8() + '\0' + suffix);
        bindBytes(s, 6, suffix);
        bindBytes(s, 7, tree);
        step(s);
        lastFile = sqlite3_last_insert_rowid(database);
    }
    lastIdentity = key;
    return lastFile;
}

ResultHit ResultStore::append(const QJsonObject &match)
{
    const qint64 file = ensureFile(match);
    const qint64 ordinal = hitCount(file);
    const bool filename = match.value("kind") == "filename";
    auto *s = query("INSERT INTO matches SELECT id,epoch,hits,CASE WHEN ? THEN names END,? FROM files WHERE id=?");
    sqlite3_bind_int(s, 1, filename);
    bindBytes(s, 2, QJsonDocument(match).toJson(QJsonDocument::Compact));
    sqlite3_bind_int64(s, 3, file);
    step(s);
    s = query("UPDATE files SET hits=hits+1,names=names+? WHERE id=?");
    sqlite3_bind_int(s, 1, filename);
    sqlite3_bind_int64(s, 2, file);
    step(s);
    if (ordinal == 0) {
        // Sort indexes depend on active membership, not on every hit count.
        s = query("UPDATE files SET active=1 WHERE id=?");
        sqlite3_bind_int64(s, 1, file);
        step(s);
        ++documents;
    }
    ++occurrences;
    advanceBatch();
    return {file, ordinal};
}

qint64 ResultStore::discard(const QJsonObject &event)
{
    const auto key = identity(event);
    auto *s = query("SELECT id,hits,names FROM files WHERE identity=? AND active=1");
    bindText(s, 1, key);
    if (!step(s)) return 0;
    const qint64 file = sqlite3_column_int64(s, 0);
    const qint64 total = sqlite3_column_int64(s, 1);
    const qint64 remaining = sqlite3_column_int64(s, 2);
    sqlite3_reset(s);
    // A file can fail after millions of provisional matches. Withdraw them
    // atomically by advancing its generation. Its independently indexed name
    // matches become the new prefix. Obsolete pages leave with the search.
    s = query("UPDATE files SET epoch=epoch+1,prefix=names,hits=names,active=(names>0) WHERE id=?");
    sqlite3_bind_int64(s, 1, file);
    step(s);
    if (!remaining) --documents;
    occurrences -= total - remaining;
    lastFile = 0;
    advanceBatch();
    return file;
}

void ResultStore::appendError(const QJsonObject &error)
{
    auto *s = query("INSERT INTO errors(data) VALUES(?)");
    bindBytes(s, 1, QJsonDocument(error).toJson(QJsonDocument::Compact));
    step(s);
    ++problems;
    advanceBatch();
}

QList<QJsonObject> ResultStore::errors(qint64 first, int size) const
{
    auto *s = query("SELECT data FROM errors WHERE id>? ORDER BY id LIMIT ?");
    sqlite3_bind_int64(s, 1, first);
    sqlite3_bind_int(s, 2, size);
    QList<QJsonObject> values;
    while (step(s)) values.append(object(s));
    return values;
}

QList<ResultFile> ResultStore::files(ResultOrder order, qint64 first, int size) const
{
    const auto col = orderColumn(order);
    auto *s = query("SELECT id,path,absolute,dirs," + col + ",hits FROM files WHERE active=1 ORDER BY " + col + " LIMIT ? OFFSET ?");
    sqlite3_bind_int(s, 1, size);
    sqlite3_bind_int64(s, 2, first);
    QList<ResultFile> result;
    while (step(s)) {
        const auto dirs = blob(s, 3);
        result.append({sqlite3_column_int64(s, 0), string(s, 1), string(s, 2),
            dirs.isEmpty() ? QList<QByteArray>{} : dirs.split('/'), blob(s, 4), sqlite3_column_int64(s, 5)});
    }
    return result;
}

QList<QJsonObject> ResultStore::hits(qint64 file, qint64 first, int size) const
{
    auto *s = query("SELECT epoch,prefix FROM files WHERE id=?");
    sqlite3_bind_int64(s, 1, file);
    if (!step(s)) return {};
    const auto epoch = sqlite3_column_int64(s, 0);
    const qint64 prefix = sqlite3_column_int64(s, 1);
    sqlite3_reset(s);
    QList<QJsonObject> result;
    if (first < prefix) {
        const qint64 amount = qMin(qint64(size), prefix - first);
        s = query("SELECT data FROM matches WHERE file=? AND name_ordinal>=? ORDER BY name_ordinal LIMIT ?");
        sqlite3_bind_int64(s, 1, file);
        sqlite3_bind_int64(s, 2, first);
        sqlite3_bind_int64(s, 3, amount);
        while (step(s)) result.append(object(s));
        first += amount;
    }
    const auto remaining = size - result.size();
    if (remaining > 0) {
        s = query("SELECT data FROM matches WHERE file=? AND epoch=? AND ordinal>=? ORDER BY ordinal LIMIT ?");
        sqlite3_bind_int64(s, 1, file);
        sqlite3_bind_int64(s, 2, epoch);
        sqlite3_bind_int64(s, 3, first);
        sqlite3_bind_int64(s, 4, remaining);
        while (step(s)) result.append(object(s));
    }
    return result;
}

qint64 ResultStore::hitCount(qint64 file) const
{
    auto *s = query("SELECT hits FROM files WHERE id=?");
    sqlite3_bind_int64(s, 1, file);
    const auto value = step(s) ? sqlite3_column_int64(s, 0) : 0;
    sqlite3_reset(s);
    return value;
}

qint64 ResultStore::filenameOrdinal(ResultHit hit) const
{
    if (hit.ordinal < 0) return -1;
    auto *s = query("SELECT CASE WHEN ?<prefix THEN ? ELSE "
        "(SELECT name_ordinal FROM matches m WHERE m.file=f.id AND m.epoch=f.epoch AND m.ordinal=?) END "
        "FROM files f WHERE id=?");
    sqlite3_bind_int64(s, 1, hit.ordinal);
    sqlite3_bind_int64(s, 2, hit.ordinal);
    sqlite3_bind_int64(s, 3, hit.ordinal);
    sqlite3_bind_int64(s, 4, hit.file);
    const auto ordinal = step(s) && sqlite3_column_type(s, 0) != SQLITE_NULL ? sqlite3_column_int64(s, 0) : -1;
    sqlite3_reset(s);
    return ordinal;
}

qint64 ResultStore::rank(ResultOrder order, qint64 file) const
{
    const auto col = orderColumn(order);
    auto *s = query("SELECT count(*) FROM files WHERE active=1 AND " + col + " < (SELECT " + col + " FROM files WHERE id=?)");
    sqlite3_bind_int64(s, 1, file);
    step(s);
    const auto v = sqlite3_column_int64(s, 0);
    sqlite3_reset(s);
    return v;
}

qint64 ResultStore::fileAt(ResultOrder order, qint64 rank) const
{
    auto *s = query("SELECT id FROM files WHERE active=1 ORDER BY " + orderColumn(order) + " LIMIT 1 OFFSET ?");
    sqlite3_bind_int64(s, 1, rank);
    const auto id = step(s) ? sqlite3_column_int64(s, 0) : 0;
    sqlite3_reset(s);
    return id;
}

qint64 ResultStore::neighbor(ResultOrder order, qint64 file, int direction) const
{
    const auto col = orderColumn(order);
    const QByteArray way = direction > 0 ? " ASC" : " DESC";
    auto *s = query("SELECT id FROM files WHERE active=1 AND " + col + (direction > 0 ? " > " : " < ") +
        "(SELECT " + col + " FROM files WHERE id=?) ORDER BY " + col + way + " LIMIT 1");
    sqlite3_bind_int64(s, 1, file);
    if (!step(s)) {
        s = query("SELECT id FROM files WHERE active=1 ORDER BY " + col + way + " LIMIT 1");
        if (!step(s)) return 0;
    }
    const auto id = sqlite3_column_int64(s, 0);
    sqlite3_reset(s);
    return id;
}
}
