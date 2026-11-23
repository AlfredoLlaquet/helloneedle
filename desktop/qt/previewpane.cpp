// SPDX-License-Identifier: GPL-3.0-or-later
#include "previewpane.h"
#include "language.h"
#include "pdfpane.h"
#include <QDesktopServices>
#include <QFileInfo>
#include <QFontDatabase>
#include <QHBoxLayout>
#include <QJsonArray>
#include <QLabel>
#include <QPushButton>
#include <QStackedWidget>
#include <QTextBrowser>
#include <QTextDocumentFragment>
#include <QUrl>
#include <QVBoxLayout>

namespace {
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
    auto *bar = new QHBoxLayout;
    name = new QLabel(this);
    name->setObjectName("previewTitle");
    name->setTextFormat(Qt::PlainText);
    name->setSizePolicy(QSizePolicy::Ignored, QSizePolicy::Preferred);
    QFont heading = name->font();
    heading.setBold(true);
    name->setFont(heading);
    bar->addWidget(name, 1);
    toggle = new QPushButton(this);
    toggle->setObjectName("pdfTextButton");
    bar->addWidget(toggle);
    open = new QPushButton(this);
    open->setObjectName("openOriginalButton");
    bar->addWidget(open);
    layout->addLayout(bar);
    place = new QLabel(this);
    place->setObjectName("previewLocation");
    place->setTextFormat(Qt::PlainText);
    place->setSizePolicy(QSizePolicy::Ignored, QSizePolicy::Preferred);
    layout->addWidget(place);
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
    connect(toggle, &QPushButton::clicked, this, [this] { chooseView(stack->currentWidget() != pdf); });
    clear();
    retranslate();
}

void PreviewPane::clear()
{
    worker.stop();
    pdf->clear();
    hasPdf = false;
    received = false;
    match = {};
    open->setEnabled(false);
    toggle->hide();
    place->clear();
    place->hide();
    name->clear();
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
    if (match.isEmpty()) name->setText(ui("Preview"));
    if (!match.isEmpty()) place->setText(locationLabel(match.value("location").toObject(), match.value("label").toString()));
    open->setToolTip(ui("Open this file in its usual application."));
    pdf->retranslate();
    toggle->setText(stack->currentWidget() == pdf ? ui("Show text") : ui("Show page"));
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
    name->setText(QFileInfo(match.value("path").toString()).fileName());
    name->setToolTip(match.value("path").toString());
    place->setText(locationLabel(match.value("location").toObject(), match.value("label").toString()));
    place->setVisible(!place->text().isEmpty());
    open->setEnabled(true);
    text->setPlaceholderText(ui("Loading preview…"));
    QJsonObject request;
    for (const auto *key : {"path", "pathBytes", "pathEncoding", "location", "mode", "revision"})
        if (match.contains(key)) request.insert(key, match.value(key));
    worker.start("preview", request, [this](const QJsonObject &data) {
        const auto type = data.value("type").toString();
        if (type == "preview") { received = true; present(data); }
        else if (type == "error") { received = true; displayError(workerError(data)); }
    }, [this](const QString &error) {
        if (!received) displayError(error.isEmpty() ? ui("The worker returned an unreadable response.") : error);
    });
}

void PreviewPane::present(const QJsonObject &data)
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
    if (selectedStart >= 0) {
        const int begin = qBound(0, match.value("blockStart").toInt(match.value("start").toInt()), selectedSize);
        const int length = qBound(0, match.value("blockLength").toInt(match.value("length").toInt()), selectedSize - begin);
        QTextCursor target(text->document());
        target.setPosition(selectedStart + (match.value("kind") == "filename" ? 0 : begin));
        text->setTextCursor(target);
        if (length && match.value("kind") != "filename") {
            target.setPosition(selectedStart + begin + length, QTextCursor::KeepAnchor);
            QTextEdit::ExtraSelection highlight;
            highlight.cursor = target;
            highlight.format.setBackground(text->palette().brush(QPalette::Highlight));
            highlight.format.setForeground(text->palette().brush(QPalette::HighlightedText));
            text->setExtraSelections({highlight});
        }
        text->ensureCursorVisible();
    }
    hasPdf = kind == "pdf" && !selectedSpot.isEmpty();
    toggle->setVisible(hasPdf);
    if (hasPdf) pdf->open(match, selectedSpot);
    chooseView(hasPdf);
}
}

