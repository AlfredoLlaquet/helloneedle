// SPDX-License-Identifier: GPL-3.0-or-later
#include "previewpane.h"
#include "buttonicons.h"
#include "language.h"
#include "pdfpane.h"
#include <QApplication>
#include <QClipboard>
#include <QDesktopServices>
#include <QDir>
#include <QFileDialog>
#include <QFileInfo>
#include <QFontDatabase>
#include <QHBoxLayout>
#include <QJsonArray>
#include <QLabel>
#include <QMimeData>
#include <QPainter>
#include <QPlainTextEdit>
#include <QPushButton>
#include <QResizeEvent>
#include <QSaveFile>
#include <QScrollBar>
#include <QStackedWidget>
#include <QTextBrowser>
#include <QTextDocumentFragment>
#include <QTextFrame>
#include <QTextLayout>
#include <QTemporaryFile>
#include <QTimer>
#include <QUrl>
#include <QVBoxLayout>
#include <QtMath>
#include <limits>
#include <memory>

namespace {
constexpr qint64 partSize = 128 * 1024; // UTF-16 units, per interactive view

class DirectoryLine final : public QPlainTextEdit {
public:
    explicit DirectoryLine(QWidget *parent) : QPlainTextEdit(parent) {
        setReadOnly(true);
        setFrameStyle(QFrame::NoFrame);
        setWordWrapMode(QTextOption::WrapAnywhere);
        setHorizontalScrollBarPolicy(Qt::ScrollBarAlwaysOff);
        setVerticalScrollBarPolicy(Qt::ScrollBarAsNeeded);
        document()->setDocumentMargin(0);
        viewport()->setAutoFillBackground(false);
        QSizePolicy policy(QSizePolicy::Ignored, QSizePolicy::Preferred);
        policy.setHeightForWidth(true);
        setSizePolicy(policy);
    }
    int heightForWidth(int width) const override {
        QTextLayout layout(toPlainText(), font(), this);
        QTextOption options;
        options.setWrapMode(QTextOption::WrapAnywhere);
        layout.setTextOption(options);
        layout.beginLayout();
        qreal height = 0;
        for (;;) {
            auto line = layout.createLine();
            if (!line.isValid()) break;
            line.setLineWidth(qMax(1, width - 2));
            height += line.height();
        }
        layout.endLayout();
        return qMax(fontMetrics().height(), qCeil(height)) + 2;
    }
    QSize sizeHint() const override { return {0, heightForWidth(width())}; }
    QSize minimumSizeHint() const override {
        return {0, qMin(heightForWidth(width()), QPlainTextEdit::minimumSizeHint().height())};
    }
protected:
    void resizeEvent(QResizeEvent *event) override {
        QPlainTextEdit::resizeEvent(event);
        if (event->size().width() != event->oldSize().width()) updateGeometry();
    }
};

class PreviewStack final : public QStackedWidget {
public:
    using QStackedWidget::QStackedWidget;

    QSize minimumSizeHint() const override
    {
        auto hint = QStackedWidget::minimumSizeHint();
        if (const auto *text = qobject_cast<QTextBrowser *>(currentWidget())) {
            // The text view can scroll. Leave the hidden PDF toolbar's height
            // available for the title and directory in a short stacked window.
            hint.setHeight(text->fontMetrics().lineSpacing() + 2 * text->frameWidth());
        }
        return hint;
    }
};

// Native clipboards request their own representation when another application
// pastes. Keep pending data on disk, independent of the preview and its lifetime.
class ClipboardMatch final : public QMimeData {
public:
    explicit ClipboardMatch(std::shared_ptr<QTemporaryFile> file) : source(std::move(file)) {}
    QStringList formats() const override { return {QStringLiteral("text/plain")}; }
protected:
    QVariant retrieveData(const QString &mime, QMetaType type) const override {
        if (mime != QStringLiteral("text/plain")) return {};
        const auto size = source->size();
        if (size == 0) return QString();
        if (size < 0 || size > std::numeric_limits<qsizetype>::max()) return {};
        uchar *mapped = source->map(0, size);
        if (!mapped) return {};
        const auto *data = reinterpret_cast<const char *>(mapped);
        QVariant result;
        try {
            result = type == QMetaType::fromType<QString>()
                ? QVariant(QString::fromUtf8(data, size)) : QVariant(QByteArray(data, size));
        } catch (const std::bad_alloc &) {
            // The same range can be exported directly to a file from the preview.
        }
        source->unmap(mapped);
        return result;
    }
private:
    std::shared_ptr<QTemporaryFile> source;
};
}

namespace hn {
// Keep the full title available to accessibility and the tooltip. Long file names
// can have no spaces at all, so ordinary word wrapping can lose their extension.
class PreviewTitle final : public QLabel {
public:
    using QLabel::QLabel;

    void setTitle(const QString &title)
    {
        setText(title);
        updateMinimumHeight();
    }

    int heightForWidth(int width) const override
    {
        QTextLayout layout(text(), font(), this);
        wrap(layout, width);
        return qMax(fontMetrics().height(), qCeil(layout.boundingRect().height()));
    }
    QSize minimumSizeHint() const override { return {0, minimumHeight()}; }

protected:
    void resizeEvent(QResizeEvent *event) override
    {
        QLabel::resizeEvent(event);
        if (event->size().width() != event->oldSize().width()) updateMinimumHeight();
    }

    void changeEvent(QEvent *event) override
    {
        QLabel::changeEvent(event);
        if (event->type() == QEvent::FontChange) updateMinimumHeight();
    }

    void paintEvent(QPaintEvent *) override
    {
        QTextLayout layout(text(), font(), this);
        wrap(layout, width());
        QPainter painter(this);
        painter.setPen(palette().color(foregroundRole()));
        for (int n = 0; n < layout.lineCount(); ++n) {
            const auto line = layout.lineAt(n);
            if (n == 1 && line.textStart() + line.textLength() < text().size()) {
                const auto tail = fontMetrics().elidedText(text().mid(line.textStart()), Qt::ElideMiddle, width());
                painter.drawText(QPointF(0, line.y() + line.ascent()), tail);
            } else {
                line.draw(&painter, {});
            }
        }
    }

private:
    // A short splitter can compress a height-for-width suggestion to one line.
    // Reserve exactly the same height used by the two-line painter.
    void updateMinimumHeight() { setMinimumHeight(heightForWidth(width())); }

    static void wrap(QTextLayout &layout, int width)
    {
        QTextOption options;
        options.setWrapMode(QTextOption::WrapAtWordBoundaryOrAnywhere);
        layout.setTextOption(options);
        layout.beginLayout();
        qreal y = 0;
        for (int n = 0; n < 2; ++n) {
            auto line = layout.createLine();
            if (!line.isValid()) break;
            line.setLineWidth(qMax(1, width));
            line.setPosition({0, y});
            y += line.height();
        }
        layout.endLayout();
    }
};

class PreviewText final : public QTextBrowser {
public:
    using QTextBrowser::QTextBrowser;
    QVariant loadResource(int, const QUrl &) override { return {}; }

    void clearPreview()
    {
        pendingPosition = -1;
        clear();
        setExtraSelections({});
    }

    void centerMatch(int position)
    {
        pendingPosition = position;
        if (isVisible()) QTimer::singleShot(0, this, &PreviewText::centerPendingMatch);
    }

protected:
    void showEvent(QShowEvent *event) override
    {
        QTextBrowser::showEvent(event);
        // PDF text may be prepared while its page view is showing.
        if (pendingPosition >= 0) centerMatch(pendingPosition);
    }

private:
    int pendingPosition = -1;

    void centerPendingMatch()
    {
        if (pendingPosition < 0 || !isVisible()) return;
        QTextCursor target(document());
        target.setPosition(pendingPosition);
        pendingPosition = -1;

        auto *bar = verticalScrollBar();
        const int middle = viewport()->rect().center().y();
        const int lineY = bar->value() + cursorRect(target).center().y();
        const int documentHeight = qCeil(document()->size().height());
        const int above = qMax(0, middle - lineY);
        const int below = qMax(0, viewport()->height() - middle - (documentHeight - lineY));
        if (above || below) {
            // Supply only the missing space at the document edges. Frame margins
            // preserve the text, formatting, selection offsets and clipboard content.
            auto *frame = document()->rootFrame();
            auto format = frame->frameFormat();
            format.setTopMargin(format.topMargin() + above);
            format.setBottomMargin(format.bottomMargin() + below);
            frame->setFrameFormat(format);
        }
        bar->setValue(bar->value() + cursorRect(target).center().y() - viewport()->rect().center().y());
    }
};

PreviewPane::PreviewPane(QWidget *parent) : QWidget(parent), worker(this), transfer(this)
{
    setObjectName("previewPane");
    auto *layout = new QVBoxLayout(this);
    layout->setContentsMargins(0, 0, 0, 0);
    name = new PreviewTitle(this);
    name->setObjectName("previewTitle");
    name->setTextFormat(Qt::PlainText);
    name->setSizePolicy(QSizePolicy::Ignored, QSizePolicy::Preferred);
    name->setAlignment(Qt::AlignLeft | Qt::AlignVCenter);
    name->setWordWrap(true);
    QFont heading = name->font();
    heading.setBold(true);
    name->setFont(heading);
    layout->addWidget(name);
    auto *directoryRow = new QHBoxLayout;
    directoryLabel = new QLabel(this);
    directoryLabel->setObjectName("previewDirectoryLabel");
    directoryRow->addWidget(directoryLabel, 0, Qt::AlignTop);
    directory = new DirectoryLine(this);
    directory->setObjectName("previewDirectory");
    directoryLabel->setBuddy(directory);
    directoryRow->addWidget(directory, 1);
    layout->addLayout(directoryRow);
    auto *bar = new QHBoxLayout;
    open = new QPushButton(this);
    open->setObjectName("openOriginalButton");
    setButtonIcon(open, ButtonIcon::OpenOriginal);
    bar->addWidget(open);
    toggle = new QPushButton(this);
    toggle->setObjectName("pdfTextButton");
    bar->addWidget(toggle);
    copy = new QPushButton(this);
    copy->setObjectName("copyMatchButton");
    bar->addWidget(copy);
    bar->addStretch();
    layout->addLayout(bar);
    auto *parts = new QHBoxLayout;
    previousPart = new QPushButton(this);
    nextPart = new QPushButton(this);
    partLabel = new QLabel(this);
    save = new QPushButton(this);
    previousPart->setObjectName("previousPreviewPart");
    nextPart->setObjectName("nextPreviewPart");
    save->setObjectName("saveMatchButton");
    parts->addWidget(previousPart);
    parts->addWidget(partLabel);
    parts->addWidget(nextPart);
    parts->addWidget(save);
    parts->addStretch();
    layout->addLayout(parts);
    transferStatus = new QLabel(this);
    transferStatus->setObjectName("matchTransferStatus");
    transferStatus->setWordWrap(true);
    layout->addWidget(transferStatus);
    stack = new PreviewStack(this);
    connect(stack, &QStackedWidget::currentChanged, stack, &QWidget::updateGeometry);
    text = new PreviewText(stack);
    text->setObjectName("previewText");
    text->setOpenLinks(false);
    text->setOpenExternalLinks(false);
    text->setReadOnly(true);
    pdf = new PdfPane(stack);
    stack->addWidget(text);
    stack->addWidget(pdf);
    layout->addWidget(stack, 1);
    connect(open, &QPushButton::clicked, this, &PreviewPane::openOriginal);
    connect(copy, &QPushButton::clicked, this, [this] { transferMatch(false); });
    connect(save, &QPushButton::clicked, this, [this] { transferMatch(true); });
    connect(previousPart, &QPushButton::clicked, this, [this] { movePart(-1); });
    connect(nextPart, &QPushButton::clicked, this, [this] { movePart(1); });
    connect(toggle, &QPushButton::clicked, this, [this] {
        const bool showPage = stack->currentWidget() != pdf;
        chooseView(showPage);
        if (!showPage && pdf->currentPage() != textPage) {
            const auto original = match.value("location").toObject();
            const int page = pdf->currentPage();
            const QJsonObject location = page == original.value("page").toInt()
                ? original : QJsonObject{{"page", page}};
            text->clearPreview();
            text->setPlaceholderText(ui("Loading preview…"));
            showingMatch = page == original.value("page").toInt();
            if (showingMatch) requestMatchPart();
            else load(location, false, 0, 0);
        }
        updateParts();
    });
    clear();
    retranslate();
}

void PreviewPane::clear()
{
    worker.stop();
    pdf->clear();
    hasPdf = false;
    received = false;
    textPage = 0;
    matchPart = 0;
    shownLocation = {};
    shownEnd = 0;
    showingMatch = true;
    match = {};
    open->setEnabled(false);
    copy->hide();
    copy->setEnabled(true);
    transferStatus->hide();
    transferStatus->clear();
    toggle->hide();
    text->clearPreview();
    stack->setCurrentWidget(text);
    retranslate();
}

void PreviewPane::retranslate()
{
    directory->setAccessibleName(ui("Directory"));
    directoryLabel->setText(ui("Directory:"));
    copy->setText(ui("Copy match"));
    save->setText(ui("Save match…"));
    previousPart->setText(ui("Previous part"));
    nextPart->setText(ui("Next part"));
    text->setAccessibleName(ui("Document preview"));
    text->setPlaceholderText(ui("Select a result to read the passage."));
    open->setText(ui("Open original"));
    updateTitle();
    open->setToolTip(ui("Open this file in its usual application."));
    pdf->retranslate();
    toggle->setText(stack->currentWidget() == pdf ? ui("Show text") : ui("Show page"));
    updateParts();
}

void PreviewPane::updateTitle()
{
    const QString path = match.value("path").toString();
    if (match.isEmpty()) {
        name->setTitle(ui("Preview"));
        name->setToolTip({});
        directory->clear();
        directory->hide();
        directoryLabel->hide();
        return;
    }
    const auto location = locationLabel(match.value("location").toObject(), match.value("label").toString());
    name->setTitle(location + QStringLiteral(" - ") + QFileInfo(path).fileName());
    name->setToolTip(path);
    const QString parent = QDir::toNativeSeparators(QFileInfo(path).absolutePath());
    if (directory->toPlainText() != parent) {
        directory->setPlainText(parent);
        directory->updateGeometry();
    }
    directory->show();
    directoryLabel->show();
}

void PreviewPane::openOriginal()
{
    if (match.isEmpty()) return;
    const auto encoded = match.value("fileUrl").toString().toLatin1();
    const QUrl url = encoded.isEmpty() ? QUrl::fromLocalFile(match.value("path").toString())
                                       : QUrl::fromEncoded(encoded);
    if (url.isLocalFile() && !QDesktopServices::openUrl(url))
        displayError(ui("The original file could not be opened."));
}

void PreviewPane::chooseView(bool page)
{
    stack->setCurrentWidget(page && hasPdf ? static_cast<QWidget *>(pdf) : static_cast<QWidget *>(text));
    toggle->setText(stack->currentWidget() == pdf ? ui("Show text") : ui("Show page"));
    updateParts();
}

void PreviewPane::displayError(const QString &message)
{
    hasPdf = false;
    toggle->hide();
    chooseView(false);
    text->clearPreview();
    text->setPlainText(message);
}

void PreviewPane::showMatch(const QJsonObject &event)
{
    clear();
    match = event;
    updateTitle();
    open->setEnabled(true);
    copy->setVisible(match.value("blockLength").toInteger() > 0);
    text->setPlaceholderText(ui("Loading preview…"));
    requestMatchPart(true);
}

void PreviewPane::requestMatchPart(bool first)
{
    showingMatch = true;
    const auto length = match.value("blockLength").toInteger();
    const auto offset = matchPart * partSize;
    auto location = match.value("location").toObject();
    if (location.contains("pageLine") && length > partSize) {
        location.insert("highlightStart", match.value("blockStart"));
        location.insert("highlightLength", length);
    }
    load(location, first, match.value("blockStart").toInteger() + offset,
         qMin(partSize, qMax<qint64>(0, length - offset)));
}

void PreviewPane::movePart(int direction)
{
    const auto length = match.value("blockLength").toInteger();
    if (showingMatch && length > partSize && matchPart + direction >= 0
        && matchPart + direction <= (length-1)/partSize) {
        matchPart += direction;
        requestMatchPart();
    } else {
        const auto from = shownLocation.value("textOffset").toInteger();
        const auto total = shownLocation.value("lineTextLength").toInteger();
        const bool inLine = total > 0 && (direction < 0 ? from > 0 : shownEnd < total);
        const int line = inLine ? shownLocation.value("pageLine").toInt()
            : direction < 0 ? shownLocation.value("firstPageLine").toInt() - 1
                            : shownLocation.value("lastPageLine").toInt() + 1;
        if (line <= 0 || line > shownLocation.value("pageLines").toInt()) return;
        QJsonObject location{{"page", textPage}, {"pageLine", line}};
        if (shownLocation.value("pdfPlain").toBool()) location.insert("pdfPlain", true);
        showingMatch = false;
        const auto start = direction > 0 ? shownEnd : qMax<qint64>(0, from - partSize);
        load(location, false, inLine ? start : 0, inLine ? partSize : 0);
    }
}

void PreviewPane::updateParts()
{
    const auto length = match.value("blockLength").toInteger();
    const auto count = length > 0 ? 1 + (length - 1) / partSize : 0;
    const bool chunked = showingMatch && count > 1;
    const auto lineLength = shownLocation.value("lineTextLength").toInteger();
    const bool before = (chunked && matchPart > 0) || (lineLength > 0 && shownLocation.value("textOffset").toInteger() > 0)
        || shownLocation.value("firstPageLine").toInt() > 1;
    const bool after = (chunked && matchPart + 1 < count) || (lineLength > 0 && shownEnd < lineLength)
        || shownLocation.value("lastPageLine").toInt() < shownLocation.value("pageLines").toInt();
    const bool show = stack->currentWidget() == text && (before || after);
    previousPart->setVisible(show);
    nextPart->setVisible(show);
    previousPart->setEnabled(before);
    nextPart->setEnabled(after);
    partLabel->setVisible(show && chunked);
    if (chunked) partLabel->setText(ui("Showing part %1 of %2").arg(matchPart + 1).arg(count));
    save->setVisible(count > 1);
}

void PreviewPane::load(const QJsonObject &location, bool first, qint64 start, qint64 length)
{
    received = false;
    previousPart->setEnabled(false);
    nextPart->setEnabled(false);
    QJsonObject request;
    for (const auto *key : {"path", "pathBytes", "pathEncoding", "mode", "revision"})
        if (match.contains(key)) request.insert(key, match.value(key));
    request.insert("location", location);
    request.insert("blockStart", start);
    request.insert("blockLength", length);
    worker.start("preview", request, [this, first](const QJsonObject &data) {
        const auto type = data.value("type").toString();
        if (type == "preview") { received = true; present(data, first); }
        else if (type == "error") { received = true; displayError(workerError(data)); }
    }, [this](const QString &error) {
        if (!received) displayError(error.isEmpty() ? ui("The worker returned an unreadable response.") : error);
    });
}

void PreviewPane::transferMatch(bool toFile)
{
    struct Transfer {
        std::shared_ptr<QTemporaryFile> spool;
        std::unique_ptr<QSaveFile> output;
        qint64 units = 0;
        bool verified = false;
        QString failure;
    };
    if (match.isEmpty()) return;
    const auto state = std::make_shared<Transfer>();
    if (toFile) {
        const QString destination = QFileDialog::getSaveFileName(this, ui("Save match…"));
        if (destination.isEmpty()) return;
        state->output = std::make_unique<QSaveFile>(destination);
        if (!state->output->open(QIODevice::WriteOnly)) state->failure = state->output->errorString();
    } else {
        state->spool = std::make_shared<QTemporaryFile>();
        if (!state->spool->open()) state->failure = state->spool->errorString();
    }
    transferStatus->show();
    if (!state->failure.isEmpty()) {
        transferStatus->setText(state->failure);
        return;
    }
    transferStatus->setText(toFile ? ui("Saving match…") : ui("Copying match…"));
    auto request = QJsonObject{{"copyMatch", true}};
    for (const auto *key : {"path", "pathBytes", "pathEncoding", "mode", "revision", "location", "blockStart", "blockLength"})
        if (match.contains(key)) request.insert(key, match.value(key));
    const auto expected = request.value("blockLength").toInteger();
    transfer.start("preview", request, [state, expected](const QJsonObject &event) {
        const auto type = event.value("type").toString();
        if (type == "error") state->failure = workerError(event);
        if (!state->failure.isEmpty()) return;
        if (type == "matchText") {
            const QString segment = event.value("text").toString();
            QFileDevice *device = state->output ? static_cast<QFileDevice *>(state->output.get()) : state->spool.get();
            const QByteArray encoded = segment.toUtf8();
            if (segment.size() > expected - state->units || device->write(encoded) != encoded.size()) {
                state->failure = ui("The matching text could not be copied.");
            } else {
                state->units += segment.size();
            }
        } else if (type == "copyReady") {
            state->verified = state->units == expected && event.value("length").toInteger() == expected;
        }
    }, [this, state](const QString &error) {
        QString reason = error.isEmpty() ? state->failure : error;
        bool success = reason.isEmpty() && state->verified;
        if (success && state->output) {
            success = state->output->commit();
            if (!success) reason = state->output->errorString();
        } else if (success) {
            success = state->spool->flush();
            if (success) QApplication::clipboard()->setMimeData(new ClipboardMatch(state->spool));
            else reason = state->spool->errorString();
        }
        if (!success && reason.isEmpty())
            reason = state->output ? ui("The matching text could not be saved.") : ui("The matching text could not be copied.");
        transferStatus->setText(success ? (state->output ? ui("Saved match") : ui("Copied match")) : reason);
        transferStatus->show();
    });
}

void PreviewPane::present(const QJsonObject &data, bool first)
{
    text->clearPreview();
    text->setPlaceholderText(ui("This document contains no previewable text."));
    const QJsonArray blocks = data.value("blocks").toArray();
    const int selected = data.value("index").toInt(-1);
    const auto kind = data.value("kind").toString();
    const bool plain = data.value("mode") == "source" || kind == "text" || kind == "txt";
    const QFont body = plain ? QFontDatabase::systemFont(QFontDatabase::FixedFont) : font();
    text->document()->setDefaultFont(body);
    QTextCursor cursor(text->document());
    QTextCharFormat normal;
    normal.setFont(body);
    int selectedStart = -1;
    int selectedSize = 0;
    QJsonObject selectedSpot;
    for (int i = 0; i < blocks.size(); ++i) {
        const auto block = blocks.at(i).toObject();
        const QString actual = block.value("text").toString();
        const QJsonObject location = block.value("location").toObject();
        if (i) cursor.insertBlock(QTextBlockFormat(), normal);
        if (location.contains("cell") || location.contains("slide")) {
            QTextCharFormat marker = normal;
            marker.setFontWeight(QFont::DemiBold);
            cursor.insertText(locationLabel(location, block.value("label").toString()), marker);
            cursor.insertText(QStringLiteral("  "), normal);
        }
        const int start = cursor.position();
        if (const auto html = block.value("html").toString(); !html.isEmpty()) {
            // The worker emits a closed vocabulary with no source URLs.
            // Verify Qt's visible-text projection before using its formatting.
            auto fragment = QTextDocumentFragment::fromHtml(html);
            if (fragment.toPlainText() == actual) cursor.insertFragment(fragment);
            else cursor.insertText(actual, normal);
        } else {
            QTextCharFormat style = normal;
            if (location.value("role") == "heading") style.setFontWeight(QFont::Bold);
            cursor.insertText(actual, style);
        }
        if (i == selected) {
            selectedStart = start;
            selectedSize = actual.size();
            selectedSpot = location;
        }
    }
    textPage = selectedSpot.value("page").toInt();
    shownLocation = selectedSpot;
    shownEnd = selectedSpot.value("textOffset").toInteger() + selectedSize;
    if (selectedStart >= 0) {
        const qint64 offset = selectedSpot.value("textOffset").toInteger();
        const qint64 start = match.value("blockStart").toInteger(match.value("start").toInteger());
        const qint64 matchLength = match.value("blockLength").toInteger(match.value("length").toInteger());
        const int begin = int(qBound(qint64(0), start - offset, qint64(selectedSize)));
        const int end = int(qBound(qint64(begin), start + matchLength - offset, qint64(selectedSize)));
        const int length = end - begin;
        const auto original = match.value("location").toObject();
        const bool sameBlock = showingMatch || (textPage > 0 && textPage == original.value("page").toInt()
                                  && selectedSpot.value("pageLine") == original.value("pageLine"));
        const bool hasMatch = sameBlock && match.value("kind") != "filename" && (length > 0 || matchLength == 0);
        QTextCursor target(text->document());
        target.setPosition(selectedStart + (hasMatch ? begin : 0));
        if (hasMatch && length) {
            // Keep the insertion point at the beginning so centering and native
            // Ctrl+C both act on the displayed selection.
            target.setPosition(selectedStart + end);
            target.setPosition(selectedStart + begin, QTextCursor::KeepAnchor);
            QTextEdit::ExtraSelection highlight;
            highlight.cursor = target;
            highlight.format.setBackground(text->palette().brush(QPalette::Highlight));
            highlight.format.setForeground(text->palette().brush(QPalette::HighlightedText));
            text->setExtraSelections({highlight});
        }
        text->setTextCursor(target);
        if (hasMatch) text->centerMatch(target.position());
    }
    if (first) {
        hasPdf = kind == "pdf" && !selectedSpot.isEmpty();
        toggle->setVisible(hasPdf);
        if (hasPdf) pdf->open(match, selectedSpot);
        chooseView(hasPdf && !selectedSpot.value("pdfPlain").toBool());
    }
    updateParts();
}
}
