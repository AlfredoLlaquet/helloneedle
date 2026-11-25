// SPDX-License-Identifier: GPL-3.0-or-later
#include "resultbrowser.h"
#include "language.h"
#include <QDialog>
#include <QDir>
#include <QDialogButtonBox>
#include <QHBoxLayout>
#include <QLabel>
#include <QPlainTextEdit>
#include <QPushButton>
#include <QSignalBlocker>
#include <QScrollBar>
#include <QTimer>
#include <QTreeWidget>
#include <QVBoxLayout>
#include <algorithm>

namespace hn {
namespace {
constexpr auto Older = "Previous matches";
constexpr auto Newer = "Next matches";
}

ResultBrowser::ResultBrowser(QWidget *parent) : QWidget(parent)
{
    auto *layout = new QVBoxLayout(this);
    layout->setContentsMargins(0, 0, 0, 0);
    tree = new QTreeWidget(this);
    tree->setObjectName("resultsView");
    tree->setHeaderHidden(true);
    tree->setUniformRowHeights(true);
    tree->setTextElideMode(Qt::ElideRight);
    tree->setHorizontalScrollBarPolicy(Qt::ScrollBarAlwaysOff);
    tree->setMinimumSize(110, 50);
    layout->addWidget(tree, 1);
    pageBar = new QWidget(this);
    auto *pages = new QHBoxLayout(pageBar);
    pages->setContentsMargins(0, 0, 0, 0);
    previous = new QPushButton(pageBar);
    previous->setObjectName("previousFilesPageButton");
    next = new QPushButton(pageBar);
    next->setObjectName("nextFilesPageButton");
    range = new QLabel(pageBar);
    range->setObjectName("filesPageLabel");
    range->setWordWrap(true);
    pages->addWidget(previous);
    pages->addWidget(range, 1);
    pages->addWidget(next);
    layout->addWidget(pageBar);
    pageBar->hide();
    refreshTimer = new QTimer(this);
    refreshTimer->setSingleShot(true);
    refreshTimer->setInterval(45);
    connect(refreshTimer, &QTimer::timeout, this, [this] { refresh(); });
    connect(previous, &QPushButton::clicked, this, [this] { showFilePage(firstFile - FilePage); });
    connect(next, &QPushButton::clicked, this, [this] { showFilePage(firstFile + FilePage); });
    connect(tree, &QTreeWidget::currentItemChanged, this, [this](QTreeWidgetItem *item) {
        if (item && item->data(0, PageStart).isValid()) return;
        ResultHit current;
        QJsonObject event;
        if (item) {
            current.file = item->data(0, FileId).toLongLong();
            current.ordinal = item->data(0, Ordinal).isValid() ? item->data(0, Ordinal).toLongLong() : -1;
            event = item->data(0, Qt::UserRole).toJsonObject();
        }
        const bool different = current != selection;
        selection = current;
        if (different && selectionChanged) selectionChanged(event);
    });
    connect(tree, &QTreeWidget::itemActivated, this, [this](QTreeWidgetItem *item) { pageAction(item); });
    connect(tree, &QTreeWidget::itemClicked, this, [this](QTreeWidgetItem *item) { pageAction(item); });
    retranslate();
}

void ResultBrowser::clearRows()
{
    const QSignalBlocker blocker(tree);
    tree->clear();
    visible.clear();
    folders.clear();
}

void ResultBrowser::reset()
{
    refreshTimer->stop();
    clearRows();
    dirty.clear();
    selection = {};
    firstFile = 0;
    failed = changedOrder = false;
    pageBar->hide();
    store.reset();
    try { store = std::make_unique<ResultStore>(); }
    catch (const StoreError &error) { fail(error); }
}

void ResultBrowser::fail(const StoreError &error)
{
    refreshTimer->stop();
    if (failed) return;
    failed = true;
    // SQLITE_FULL/IOERR can roll back a batch. Cached counts and widgets must
    // not claim those rows remain available.
    clearRows();
    selection = {};
    pageBar->hide();
    store.reset();
    if (selectionChanged) selectionChanged({});
    if (storageFailure)
        storageFailure(ui("The temporary result store could not be used.") + QLatin1Char(' ') + QString::fromUtf8(error.what()));
}

qint64 ResultBrowser::count() const { return store ? store->totalHits() : 0; }
qint64 ResultBrowser::errors() const { return store ? store->errorCount() : 0; }

void ResultBrowser::append(const QJsonObject &match)
{
    if (!store || failed) return;
    try {
        const auto documents = store->fileCount();
        const auto id = store->append(match);
        if (count() == 1) {
            select(id);
            return;
        }
        // Hidden files and hits past an already-full visible page need no new
        // widgets. The last row continues to lead to all later occurrences.
        const auto group = visible.constFind(id.file);
        if (documents != store->fileCount()) changedOrder = true;
        if (group != visible.cend() && id.ordinal <= group->first + HitPage)
            dirty.insert(id.file);
        if ((changedOrder || !dirty.isEmpty()) && !refreshTimer->isActive()) refreshTimer->start();
    } catch (const StoreError &error) { fail(error); }
}

void ResultBrowser::appendError(const QJsonObject &error)
{
    if (!store || failed) return;
    try { store->appendError(error); }
    catch (const StoreError &problem) { fail(problem); }
}

void ResultBrowser::discard(const QJsonObject &file)
{
    if (!store || failed) return;
    try {
        const auto previousCount = store->fileCount();
        const auto nextFile = selection.file ? store->neighbor(order, selection.file, 1) : 0;
        const qint64 survivingOrdinal = store->filenameOrdinal(selection);
        const qint64 id = store->discard(file);
        if (!id) return;
        const qint64 remaining = store->hitCount(id);
        const bool targeted = selection.file == id;
        const bool kept = targeted && remaining > 0
            && (selection.ordinal < 0 || survivingOrdinal >= 0);
        if (kept && selection.ordinal >= 0) selection.ordinal = survivingOrdinal;
        changedOrder |= previousCount != store->fileCount();
        if (visible.contains(id)) {
            const QSignalBlocker blocker(tree);
            removeFile(id);
        }
        if (targeted && !kept) {
            const auto replacement = remaining ? id : nextFile == id ? qint64(0) : nextFile;
            selection = {}; // The old preview is stale even if an ordinal is reused.
            if (replacement) { select({replacement, 0}); return; }
            if (selectionChanged) selectionChanged({});
        }
        refresh();
    } catch (const StoreError &error) { fail(error); }
}

void ResultBrowser::settle()
{
    refreshTimer->stop();
    if (!store || failed) return;
    try { store->flush(); refresh(); }
    catch (const StoreError &error) { fail(error); }
}

void ResultBrowser::setOrder(ResultOrder nextOrder)
{
    if (order == nextOrder) return;
    order = nextOrder;
    clearRows();
    firstFile = 0;
    changedOrder = true;
    refresh();
}

QString ResultBrowser::hitLabel(const QJsonObject &match) const
{
    const auto description = locationLabel(match["location"].toObject(), match["label"].toString());
    QString snippet = match["text"].toString();
    snippet.replace(QLatin1Char('\n'), QLatin1Char(' '));
    return description.isEmpty() ? snippet : description + QStringLiteral("   ") + snippet;
}

void ResultBrowser::retranslate()
{
    tree->setAccessibleName(ui("Search results"));
    previous->setText(QStringLiteral("◀"));
    next->setText(QStringLiteral("▶"));
    previous->setToolTip(ui("Previous files"));
    previous->setAccessibleName(ui("Previous files"));
    next->setToolTip(ui("Next files"));
    next->setAccessibleName(ui("Next files"));
    for (const auto &group : visible) {
        for (int i = 0; i < group.item->childCount(); ++i) {
            auto *item = group.item->child(i);
            if (item->data(0, PageStart).isValid())
                item->setText(0, ui(item->data(0, PageStart).toLongLong() < group.first ? Older : Newer));
            else item->setText(0, hitLabel(item->data(0, Qt::UserRole).toJsonObject()));
        }
    }
    refresh();
}

void ResultBrowser::insertSorted(QTreeWidgetItem *parent, QTreeWidgetItem *item)
{
    const auto key = item->data(0, OrderKey).toByteArray();
    int first = 0, last = parent->childCount();
    while (first < last) {
        const auto middle = first + (last - first)/2;
        if (parent->child(middle)->data(0, OrderKey).toByteArray() < key) first = middle + 1;
        else last = middle;
    }
    parent->insertChild(first, item);
}

QTreeWidgetItem *ResultBrowser::folderFor(const ResultFile &file)
{
    auto *parent = tree->invisibleRootItem();
    if (order != ResultOrder::Tree) return parent;
    const auto parts = file.path.split('/');
    QByteArray key;
    for (qsizetype i = 0; i < file.directories.size(); ++i) {
        key += '/' + file.directories[i];
        auto *folder = folders.value(key);
        if (!folder) {
            folder = new QTreeWidgetItem(QStringList{parts[i]});
            folder->setData(0, FolderKey, key);
            folder->setData(0, OrderKey, '\1' + parts[i].toUtf8() + '\0' + file.directories[i]);
            folder->setToolTip(0, parts.mid(0, i + 1).join('/'));
            insertSorted(parent, folder);
            folders.insert(key, folder);
        }
        parent = folder;
    }
    return parent;
}

void ResultBrowser::removeFile(qint64 id)
{
    auto *entry = visible.take(id).item;
    if (!entry) return;
    auto *parent = entry->parent();
    delete entry;
    while (parent && parent->childCount() == 0) {
        folders.remove(parent->data(0, FolderKey).toByteArray());
        auto *nextParent = parent->parent();
        delete parent;
        parent = nextParent;
    }
}

void ResultBrowser::syncFile(const ResultFile &file)
{
    auto it = visible.find(file.id);
    if (it == visible.end()) {
        auto *parent = folderFor(file);
        const auto basename = file.path.section('/', -1);
        auto *item = new QTreeWidgetItem(QStringList{order == ResultOrder::Paths ? QDir::toNativeSeparators(file.path) : basename});
        item->setData(0, FileId, file.id);
        item->setData(0, OrderKey, order == ResultOrder::Tree ? '\2' + basename.toUtf8() + '\0' + file.order : file.order);
        item->setToolTip(0, file.absolute);
        auto font = tree->font();
        font.setBold(true);
        item->setFont(0, font);
        insertSorted(parent, item);
        it = visible.insert(file.id, {item});
    }
    auto &group = it.value();
    const qint64 wanted = selection.file == file.id && selection.ordinal >= 0
        ? (selection.ordinal / HitPage) * HitPage : group.first;
    if (wanted != group.first) {
        qDeleteAll(group.item->takeChildren());
        group.first = group.last = wanted;
        group.more = nullptr;
        group.total = 0;
    }
    if (group.total != file.count) syncRows(group, file);
}

void ResultBrowser::syncRows(VisibleFile &group, const ResultFile &file)
{
    auto marker = [&](const char *text, qint64 jump) {
        auto *row = new QTreeWidgetItem(group.item, {ui(text)});
        row->setData(0, FileId, file.id);
        row->setData(0, PageStart, jump);
        row->setToolTip(0, ui(text));
        return row;
    };
    if (group.first > 0 && !group.item->childCount()) marker(Older, group.first - HitPage);
    const qint64 end = qMin(file.count, group.first + HitPage);
    if (group.last < end) {
        const auto values = store->hits(file.id, group.last, int(end - group.last));
        for (const auto &event : values) {
            auto *item = new QTreeWidgetItem(group.item, {hitLabel(event)});
            item->setData(0, Qt::UserRole, event);
            item->setData(0, FileId, file.id);
            item->setData(0, Ordinal, group.last++);
            item->setToolTip(0, event["text"].toString());
        }
    }
    if (end < file.count && !group.more) group.more = marker(Newer, end);
    group.total = file.count;
}

void ResultBrowser::refresh()
{
    if (!store || failed) return;
    try {
        const QSignalBlocker blocker(tree);
        const auto documents = store->fileCount();
        if (selection.file && (changedOrder || !visible.contains(selection.file)))
            firstFile = store->rank(order, selection.file) / FilePage * FilePage;
        firstFile = qMin(firstFile, qMax(qint64(0), documents - 1) / FilePage * FilePage);
        const auto entries = store->files(order, firstFile, FilePage);
        QSet<qint64> keep;
        for (const auto &entry : entries) keep.insert(entry.id);
        const auto old = visible.keys();
        for (const auto id : old)
            if (!keep.contains(id)) removeFile(id);
        for (const auto &entry : entries) syncFile(entry);
        if (selection.file) selectRow(selection);
        range->setText(ui("Files %1–%2 of %3").arg(firstFile + 1).arg(qMin(documents, firstFile + FilePage)).arg(documents));
        pageBar->setVisible(documents > FilePage);
        previous->setEnabled(firstFile > 0);
        next->setEnabled(firstFile + FilePage < documents);
        dirty.clear();
        changedOrder = false;
    } catch (const StoreError &error) { fail(error); }
}

void ResultBrowser::selectRow(ResultHit hit, bool force)
{
    const auto group = visible.value(hit.file);
    if (!group.item) return;
    QTreeWidgetItem *item = group.item;
    if (hit.ordinal >= 0) {
        if (hit.ordinal < group.first || hit.ordinal >= group.last) return;
        item = group.item->child(int(hit.ordinal - group.first) + (group.first > 0 ? 1 : 0));
    }
    if (!item || (!force && tree->currentItem() == item && item->isSelected())) return;
    for (auto *parent = item->parent(); parent; parent = parent->parent()) parent->setExpanded(true);
    tree->setCurrentItem(item, 0, QItemSelectionModel::ClearAndSelect | QItemSelectionModel::Rows);
    tree->scrollToItem(item);
}

void ResultBrowser::select(ResultHit hit)
{
    if (!store || !hit.file || failed) return;
    const bool different = hit != selection;
    selection = hit;
    // Explicit navigation can cross either the document page or a hit page.
    if (!visible.contains(hit.file)) changedOrder = true;
    refresh();
    if (failed) return;
    {
        const QSignalBlocker blocker(tree);
        selectRow(hit, true);
    }
    if (different && selectionChanged) selectionChanged(selectedMatch());
}

QJsonObject ResultBrowser::selectedMatch() const
{
    const auto *item = tree->currentItem();
    return item ? item->data(0, Qt::UserRole).toJsonObject() : QJsonObject{};
}

void ResultBrowser::showFilePage(qint64 offset)
{
    if (!store || failed || offset < 0 || offset >= store->fileCount()) return;
    try { select({store->fileAt(order, offset), 0}); }
    catch (const StoreError &error) { fail(error); }
}

void ResultBrowser::pageAction(QTreeWidgetItem *item)
{
    if (!store || failed || !item || !item->data(0, PageStart).isValid()) return;
    const ResultHit target{item->data(0, FileId).toLongLong(), item->data(0, PageStart).toLongLong()};
    // Do not destroy the activated item while Qt is dispatching mouse/keyboard events.
    QTimer::singleShot(0, this, [this, target] { select(target); });
}

void ResultBrowser::navigate(int direction, bool byDocument)
{
    if (!store || failed || !count()) return;
    try {
        const qint64 file = selection.file;
        if (file && !byDocument) {
            const auto index = selection.ordinal + direction;
            if (index >= 0 && index < store->hitCount(file)) { select({file, index}); return; }
        }
        const auto target = store->neighbor(order, file, direction);
        const auto ordinal = (byDocument || direction > 0) ? 0 : store->hitCount(target) - 1;
        select({target, ordinal});
    } catch (const StoreError &error) { fail(error); }
}

void ResultBrowser::showErrors()
{
    if (!store || !errors()) return;
    QDialog dialog(this);
    dialog.setWindowTitle(ui("Search details"));
    auto *layout = new QVBoxLayout(&dialog);
    auto *content = new QPlainTextEdit(&dialog);
    content->setObjectName("errorDetailsText");
    content->setReadOnly(true);
    layout->addWidget(content);
    auto *pager = new QHBoxLayout;
    auto *back = new QPushButton(ui("Previous errors"), &dialog);
    auto *label = new QLabel(&dialog);
    auto *forward = new QPushButton(ui("Next errors"), &dialog);
    back->setObjectName("previousErrorsButton");
    forward->setObjectName("nextErrorsButton");
    pager->addWidget(back);
    pager->addWidget(label, 1, Qt::AlignCenter);
    pager->addWidget(forward);
    layout->addLayout(pager);
    constexpr int pageSize = 64;
    qint64 start = 0, displayedCount = -1, renderedStart = -1;
    auto populate = [&] {
        try {
            const auto total = errors();
            QStringList lines;
            for (const auto &record : store->errors(start, pageSize)) {
                auto line = workerError(record);
                const auto detail = record["message"].toString().trimmed();
                if (!detail.isEmpty() && !line.contains(detail)) line += QLatin1Char('\n') + detail;
                const auto path = record["path"].toString();
                lines << (path.isEmpty() ? line : path + QLatin1Char('\n') + line);
            }
            const auto pageText = lines.join(QStringLiteral("\n\n"));
            if (start != renderedStart) {
                content->setPlainText(pageText);
                renderedStart = start;
            } else if (content->toPlainText() != pageText) {
                // A new diagnostic may arrive while someone is copying a path.
                const auto previousCursor = content->textCursor();
                const auto anchor = previousCursor.anchor();
                const auto position = previousCursor.position();
                const auto scroll = content->verticalScrollBar()->value();
                content->setPlainText(pageText);
                auto cursor = content->textCursor();
                cursor.setPosition(anchor);
                cursor.setPosition(position, QTextCursor::KeepAnchor);
                content->setTextCursor(cursor);
                content->verticalScrollBar()->setValue(scroll);
            }
            displayedCount = total;
            label->setText(ui("Errors %1–%2 of %3").arg(start + 1).arg(qMin(total, start + pageSize)).arg(total));
            back->setEnabled(start > 0);
            forward->setEnabled(start + pageSize < total);
        } catch (const StoreError &e) { content->setPlainText(QString::fromUtf8(e.what())); fail(e); }
    };
    connect(back, &QPushButton::clicked, &dialog, [&] { start -= pageSize; populate(); });
    connect(forward, &QPushButton::clicked, &dialog, [&] { start += pageSize; populate(); });
    QTimer poll;
    connect(&poll, &QTimer::timeout, &dialog, [&] {
        if (!store || failed) dialog.reject();
        else if (displayedCount != errors()) populate();
    });
    poll.start(300);
    populate();
    auto *buttons = new QDialogButtonBox(&dialog);
    buttons->addButton(ui("Close"), QDialogButtonBox::RejectRole);
    connect(buttons, &QDialogButtonBox::rejected, &dialog, &QDialog::reject);
    layout->addWidget(buttons);
    dialog.resize(590, 360);
    dialog.exec();
}
}
