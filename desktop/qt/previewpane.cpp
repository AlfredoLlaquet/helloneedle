// SPDX-License-Identifier: GPL-3.0-or-later
#include "previewpane.h"
#include "buttonicons.h"
#include "language.h"
#include "pdfpane.h"
#include <QDesktopServices>
#include <QFileInfo>
#include <QFontDatabase>
#include <QHBoxLayout>
#include <QJsonArray>
#include <QLabel>
#include <QPainter>
#include <QPushButton>
#include <QStackedWidget>
#include <QTextBrowser>
#include <QTextDocumentFragment>
#include <QTextLayout>
#include <QUrl>
#include <QVBoxLayout>
#include <QtMath>

namespace {
// Keep the full title available to accessibility and the tooltip. Long file names
// can have no spaces at all, so ordinary word wrapping can lose their extension.
class PreviewTitle final : public QLabel {
public:
    using QLabel::QLabel;

    int heightForWidth(int width) const override
    {
        QTextLayout layout(text(), font(), this);
        wrap(layout, width);
        return qMax(fontMetrics().height(), qCeil(layout.boundingRect().height()));
    }
    QSize minimumSizeHint() const override { return {0, fontMetrics().height()}; }

protected:
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

class LocalPreview final : public QTextBrowser {
public:
    using QTextBrowser::QTextBrowser;
    QVariant loadResource(int, const QUrl &) override { return {}; }
};
}

namespace hn {
PreviewPane::PreviewPane(QWidget *parent) : QWidget(parent), worker(this)
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
    auto *bar = new QHBoxLayout;
    open = new QPushButton(this);
    open->setObjectName("openOriginalButton");
    setButtonIcon(open, ButtonIcon::OpenOriginal);
    bar->addWidget(open);
    toggle = new QPushButton(this);
    toggle->setObjectName("pdfTextButton");
    bar->addWidget(toggle);
    bar->addStretch();
    layout->addLayout(bar);
    stack = new QStackedWidget(this);
    text = new LocalPreview(stack);
    text->setObjectName("previewText");
    text->setOpenLinks(false);
    text->setOpenExternalLinks(false);
    text->setReadOnly(true);
    pdf = new PdfPane(stack);
    stack->addWidget(text);
    stack->addWidget(pdf);
    layout->addWidget(stack, 1);
    connect(open, &QPushButton::clicked, this, &PreviewPane::openOriginal);
    connect(toggle, &QPushButton::clicked, this, [this] {
        const bool showPage = stack->currentWidget() != pdf;
        chooseView(showPage);
        if (!showPage && pdf->currentPage() != textPage) {
            const auto original = match.value("location").toObject();
            const int page = pdf->currentPage();
            const QJsonObject location = page == original.value("page").toInt()
                ? original : QJsonObject{{"page", page}};
            text->clear();
            text->setExtraSelections({});
            text->setPlaceholderText(ui("Loading preview…"));
            load(location, false);
        }
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
    match = {};
    open->setEnabled(false);
    toggle->hide();
    text->clear();
    text->setExtraSelections({});
    stack->setCurrentWidget(text);
    retranslate();
}

void PreviewPane::retranslate()
{
    text->setAccessibleName(ui("Document preview"));
    text->setPlaceholderText(ui("Select a result to read the passage."));
    open->setText(ui("Open original"));
    updateTitle();
    open->setToolTip(ui("Open this file in its usual application."));
    pdf->retranslate();
    toggle->setText(stack->currentWidget() == pdf ? ui("Show text") : ui("Show page"));
}

void PreviewPane::updateTitle()
{
    const QString path = match.value("path").toString();
    if (match.isEmpty()) {
        name->setText(ui("Preview"));
        name->setToolTip({});
        return;
    }
    const auto location = locationLabel(match.value("location").toObject(), match.value("label").toString());
    name->setText(location + QStringLiteral(" - ") + QFileInfo(path).fileName());
    name->setToolTip(path);
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
}

void PreviewPane::displayError(const QString &message)
{
    hasPdf = false;
    toggle->hide();
    chooseView(false);
    text->setExtraSelections({});
    text->setPlainText(message);
}

void PreviewPane::showMatch(const QJsonObject &event)
{
    clear();
    match = event;
    updateTitle();
    open->setEnabled(true);
    text->setPlaceholderText(ui("Loading preview…"));
    load(match.value("location").toObject(), true);
}

void PreviewPane::load(const QJsonObject &location, bool first)
{
    received = false;
    QJsonObject request;
    for (const auto *key : {"path", "pathBytes", "pathEncoding", "mode", "revision", "blockStart"})
        if (match.contains(key)) request.insert(key, match.value(key));
    request.insert("location", location);
    worker.start("preview", request, [this, first](const QJsonObject &data) {
        const auto type = data.value("type").toString();
        if (type == "preview") { received = true; present(data, first); }
        else if (type == "error") { received = true; displayError(workerError(data)); }
    }, [this](const QString &error) {
        if (!received) displayError(error.isEmpty() ? ui("The worker returned an unreadable response.") : error);
    });
}

void PreviewPane::present(const QJsonObject &data, bool first)
{
    text->clear();
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
    if (selectedStart >= 0) {
        const qint64 offset = selectedSpot.value("textOffset").toInteger();
        const qint64 start = match.value("blockStart").toInteger(match.value("start").toInteger());
        const qint64 matchLength = match.value("blockLength").toInteger(match.value("length").toInteger());
        const int begin = int(qBound(qint64(0), start - offset, qint64(selectedSize)));
        const int end = int(qBound(qint64(begin), start + matchLength - offset, qint64(selectedSize)));
        const int length = end - begin;
        QTextCursor target(text->document());
        target.setPosition(selectedStart + (match.value("kind") == "filename" ? 0 : begin));
        text->setTextCursor(target);
        const int originalPage = match.value("location").toObject().value("page").toInt();
        if (length && match.value("kind") != "filename" && textPage == originalPage) {
            target.setPosition(selectedStart + begin + length, QTextCursor::KeepAnchor);
            QTextEdit::ExtraSelection highlight;
            highlight.cursor = target;
            highlight.format.setBackground(text->palette().brush(QPalette::Highlight));
            highlight.format.setForeground(text->palette().brush(QPalette::HighlightedText));
            text->setExtraSelections({highlight});
        }
        text->ensureCursorVisible();
    }
    if (first) {
        hasPdf = kind == "pdf" && !selectedSpot.isEmpty();
        toggle->setVisible(hasPdf);
        if (hasPdf) pdf->open(match, selectedSpot);
        chooseView(hasPdf);
    }
}
}
