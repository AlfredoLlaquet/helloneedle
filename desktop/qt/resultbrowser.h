// SPDX-License-Identifier: GPL-3.0-or-later
#pragma once
#include "resultstore.h"
#include <QSet>
#include <QWidget>
#include <functional>
#include <memory>

class QLabel;
class QPushButton;
class QTimer;
class QTreeWidget;
class QTreeWidgetItem;

namespace hn {
// Only a page of documents and a page of occurrences per document are widgets.
// All identifiers and navigation beyond those pages refer to the disk index.
class ResultBrowser final : public QWidget {
public:
    explicit ResultBrowser(QWidget *parent);
    void reset();
    void append(const QJsonObject &match);
    void appendError(const QJsonObject &error);
    void discard(const QJsonObject &file);
    void settle();
    void showErrors();
    void setOrder(ResultOrder order);
    void retranslate();
    QJsonObject selectedMatch() const;
    void navigate(int direction, bool byDocument);
    qint64 count() const;
    qint64 errors() const;

    std::function<void(const QJsonObject &)> selectionChanged;
    std::function<void(const QString &)> storageFailure;
private:
    static constexpr int FilePage = 64;
    static constexpr int HitPage = 64;
    enum Role { FileId = Qt::UserRole + 1, Ordinal, OrderKey, FolderKey, PageStart };
    struct VisibleFile {
        QTreeWidgetItem *item = nullptr;
        qint64 first = 0;
        qint64 last = 0;
        qint64 total = 0;
        QTreeWidgetItem *more = nullptr;
    };
    void fail(const StoreError &error);
    void select(ResultHit hit);
    void selectRow(ResultHit hit, bool force = false);
    void showFilePage(qint64 offset);
    void refresh();
    void syncFile(const ResultFile &file);
    void syncRows(VisibleFile &group, const ResultFile &file);
    void removeFile(qint64 file);
    void insertSorted(QTreeWidgetItem *parent, QTreeWidgetItem *item);
    QTreeWidgetItem *folderFor(const ResultFile &file);
    void pageAction(QTreeWidgetItem *item);
    void clearRows();
    QString hitLabel(const QJsonObject &hit) const;

    ResultOrder order = ResultOrder::Names;
    std::unique_ptr<ResultStore> store;
    QTreeWidget *tree;
    QWidget *pageBar;
    QPushButton *previous;
    QPushButton *next;
    QLabel *range;
    QTimer *refreshTimer;
    QHash<qint64, VisibleFile> visible;
    QHash<QByteArray, QTreeWidgetItem *> folders;
    QSet<qint64> dirty;
    qint64 firstFile = 0;
    ResultHit selection;
    bool changedOrder = false;
    bool failed = false;
};
}
