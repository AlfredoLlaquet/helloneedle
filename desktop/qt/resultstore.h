// SPDX-License-Identifier: GPL-3.0-or-later
#pragma once
#include <QHash>
#include <QJsonObject>
#include <QStringList>
#include <stdexcept>

struct sqlite3;
struct sqlite3_stmt;

namespace hn {
enum class ResultOrder { Names, Paths, Tree };

struct ResultFile {
    qint64 id = 0;
    QString path;
    QString absolute;
    // The native directory identities keep different Unix names that decode to
    // the same display text separate in the tree.
    QList<QByteArray> directories;
    QByteArray order;
    qint64 count = 0;
};

struct ResultHit {
    qint64 file = 0;
    qint64 ordinal = -1;
    bool operator==(const ResultHit &other) const { return file == other.file && ordinal == other.ordinal; }
    bool operator!=(const ResultHit &other) const { return !(*this == other); }
};

class StoreError : public std::runtime_error {
public:
    using std::runtime_error::runtime_error;
};

// SQLite's unnamed temporary database is private to this search. Its indexes
// and payloads can spill to disk, and closing the connection removes its files.
class ResultStore {
public:
    ResultStore();
    ~ResultStore();
    ResultStore(const ResultStore &) = delete;
    ResultStore &operator=(const ResultStore &) = delete;
    ResultHit append(const QJsonObject &match);
    qint64 discard(const QJsonObject &file);
    void appendError(const QJsonObject &error);
    QList<QJsonObject> errors(qint64 first, int size) const;
    QList<ResultFile> files(ResultOrder order, qint64 first, int size) const;
    QList<QJsonObject> hits(qint64 file, qint64 first, int size) const;
    qint64 rank(ResultOrder order, qint64 file) const;
    qint64 fileAt(ResultOrder order, qint64 rank) const;
    qint64 neighbor(ResultOrder order, qint64 file, int direction) const;
    qint64 hitCount(qint64 file) const;
    qint64 filenameOrdinal(ResultHit hit) const;
    qint64 fileCount() const { return documents; }
    qint64 totalHits() const { return occurrences; }
    qint64 errorCount() const { return problems; }
    void flush();

private:
    void execute(const char *sql) const;
    sqlite3_stmt *query(const QByteArray &sql) const;
    bool step(sqlite3_stmt *statement) const;
    void checked(int result) const;
    void advanceBatch();
    qint64 ensureFile(const QJsonObject &match);
    sqlite3 *database = nullptr;
    mutable QHash<QByteArray, sqlite3_stmt *> statements;
    QString lastIdentity;
    qint64 lastFile = 0;
    qint64 documents = 0;
    qint64 occurrences = 0;
    qint64 problems = 0;
    int changes = 0;
};
}
