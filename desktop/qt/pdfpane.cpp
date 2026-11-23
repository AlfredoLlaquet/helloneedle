// SPDX-License-Identifier: GPL-3.0-or-later
#include "pdfpane.h"
#include "language.h"
#include "workerclient.h"
#include <QEvent>
#include <QFile>
#include <QHBoxLayout>
#include <QJsonArray>
#include <QLabel>
#include <QPainter>
#include <QProcess>
#include <QScrollArea>
#include <QToolButton>
#include <QVBoxLayout>
#include <algorithm>
#include <cmath>
#ifdef Q_OS_UNIX
#include <fcntl.h>
#include <unistd.h>
#endif

namespace hn {
PdfPane::PdfPane(QWidget *parent) : QWidget(parent)
{
    setObjectName("pdfPane");
    auto *layout = new QVBoxLayout(this);
    layout->setContentsMargins(0, 0, 0, 0);
    auto *tools = new QHBoxLayout;
    auto button = [this, tools](const char *id) {
        auto *b = new QToolButton(this);
        b->setObjectName(id);
        tools->addWidget(b);
        return b;
    };
    previous = button("previousPageButton");
    pageLabel = new QLabel(this);
    pageLabel->setObjectName("pdfPageLabel");
    tools->addWidget(pageLabel);
    next = button("nextPageButton");
    tools->addStretch();
    zoomOut = button("zoomOutButton");
    zoomIn = button("zoomInButton");
    fit = button("fitPageButton");
    layout->addLayout(tools);
    scroller = new QScrollArea(this);
    scroller->setAlignment(Qt::AlignCenter);
    scroller->viewport()->installEventFilter(this);
    image = new QLabel;
    image->setObjectName("pdfImage");
    image->setAlignment(Qt::AlignCenter);
    image->setWordWrap(true);
    scroller->setWidget(image);
    layout->addWidget(scroller);
    connect(previous, &QToolButton::clicked, this, [this]{ goTo(page - 1); });
    connect(next, &QToolButton::clicked, this, [this]{ goTo(page + 1); });
    connect(fit, &QToolButton::clicked, this, [this]{ fitToPage = true; draw(true); });
    auto changeZoom = [this](double multiplier) {
        if (raster.isNull()) return;
        if (fitToPage) zoom = double(image->width()) / raster.width();
        zoom = std::clamp(zoom * multiplier, 0.08, 3.0);
        fitToPage = false;
        draw(true);
    };
    connect(zoomIn, &QToolButton::clicked, this, [changeZoom]{ changeZoom(1.25); });
    connect(zoomOut, &QToolButton::clicked, this, [changeZoom]{ changeZoom(0.8); });
    retranslate();
}

void PdfPane::retranslate()
{
    previous->setText(ui("Previous"));
    next->setText(ui("Next"));
    zoomIn->setText(QStringLiteral("+"));
    zoomOut->setText(QStringLiteral("−"));
    fit->setText(ui("Fit"));
    for (const auto &entry : {qMakePair(previous,"Previous page"), qMakePair(next,"Next page"),
            qMakePair(zoomIn,"Zoom in"), qMakePair(zoomOut,"Zoom out"), qMakePair(fit,"Fit page")}) {
        entry.first->setAccessibleName(ui(entry.second));
        entry.first->setToolTip(ui(entry.second));
    }
    image->setAccessibleName(ui("PDF page"));
    pageLabel->setText(ui("Page %1 of %2").arg(page).arg(pages));
}

void PdfPane::cancel()
{
    if (!job) return;
    auto *old = job;
    job = nullptr;
    old->disconnect(this);
    if (old->state() != QProcess::NotRunning) {
        connect(old, &QProcess::finished, old, &QObject::deleteLater);
        old->kill();
    } else old->deleteLater();
}

void PdfPane::clear()
{
    cancel();
    file.clear();
    hit = {};
    spot = {};
    raster = {};
    image->clear();
    page = pages = 0;
}

void PdfPane::open(const QJsonObject &match, const QJsonObject &location)
{
    clear();
    hit = match;
    spot = location;
    file = match.value("path").toString();
    const int selected = qMax(1, location.value("page").toInt(1));
    pages = qMax(selected, location.value("pageCount").toInt(selected));
    zoom = 1;
    fitToPage = true;
    goTo(selected);
}

void PdfPane::goTo(int target)
{
    if (target < 1 || target > pages) return;
    cancel();
    page = target;
    raster = {};
    image->clear();
    image->setText(ui("Loading page…"));
    image->resize(scroller->viewport()->size());
    previous->setEnabled(page > 1);
    next->setEnabled(page < pages);
    pageLabel->setText(ui("Page %1 of %2").arg(page).arg(pages));
    const auto program = findTool("pdftoppm");
    if (program.isEmpty()) {
        image->setText(ui("PDF page rendering requires Poppler (pdftoppm)."));
        return;
    }
    auto *p = new QProcess(this);
    job = p;
    p->setStandardErrorFile(QProcess::nullDevice());
    QString argument = file;
#ifdef Q_OS_UNIX
    const auto native = QByteArray::fromBase64(hit.value("pathBytes").toString().toLatin1());
    if (!native.isEmpty() && hit.value("pathEncoding").toString() == "unix" && native != QFile::encodeName(file)) {
        // QString cannot losslessly hold an arbitrary Unix filename. Hand the
        // exact file descriptor to the child without rewriting the source.
        p->setChildProcessModifier([native] {
            int fd = ::open(native.constData(), O_RDONLY);
            if (fd < 0 || ::dup2(fd, STDIN_FILENO) < 0) ::_exit(126);
            if (fd != STDIN_FILENO) ::close(fd);
        });
        argument = QStringLiteral("-");
    }
#endif
    connect(p, &QProcess::errorOccurred, this, [this, p](QProcess::ProcessError err) {
        if (p == job && err == QProcess::FailedToStart) {
            image->setText(ui("The PDF page could not be rendered."));
            cancel();
        }
    });
    connect(p, &QProcess::finished, this, [this, p](int code, QProcess::ExitStatus status) {
        if (p != job) return;
        if (code == 0 && status == QProcess::NormalExit)
            raster = QImage::fromData(p->readAllStandardOutput(), "PNG");
        cancel();
        if (raster.isNull()) image->setText(ui("The PDF page could not be rendered."));
        else draw(true);
    });
    p->start(program, {"-f", QString::number(page), "-l", QString::number(page),
                       "-singlefile", "-scale-to", "1800", "-png", argument}, QIODevice::ReadOnly);
}

void PdfPane::draw(bool centerMatch)
{
    if (raster.isNull()) return;
    QSize available = scroller->viewport()->size() - QSize(8, 8);
    const QSize size = fitToPage ? raster.size().scaled(available.expandedTo({1,1}), Qt::KeepAspectRatio)
                                 : raster.size() * zoom;
    if (size.isEmpty()) return;
    double width = spot.value("width").toDouble(), height = spot.value("height").toDouble();
    const double ratio = double(raster.width()) / raster.height();
    if (width > 0 && height > 0 && std::abs(ratio - height/width) < std::abs(ratio - width/height))
        std::swap(width, height);
    // Annotate the source raster first. Resizing a normal window never
    // changes the location recorded in the original PDF coordinate space.
    QImage annotated = raster;
    QRectF selection;
    if (page == spot.value("page").toInt() && width > 0 && height > 0 && hit.value("kind") != "filename") {
        const int begin = hit.value("blockStart").toInt(hit.value("start").toInt());
        const int end = begin + hit.value("blockLength").toInt(hit.value("length").toInt());
        QPainter painter(&annotated);
        painter.setPen(QPen(QColor(130, 66, 0), 2));
        painter.setBrush(QColor(255, 198, 49, 105));
        for (const auto &entry : spot.value("words").toArray()) {
            const QJsonObject word = entry.toObject();
            const int start = word.value("start").toInt();
            if (start >= end || start + word.value("length").toInt() <= begin) continue;
            const double sx = raster.width() / width, sy = raster.height() / height;
            const QRectF rect(word.value("x").toDouble()*sx, word.value("y").toDouble()*sy,
                              word.value("w").toDouble()*sx, word.value("h").toDouble()*sy);
            painter.drawRect(rect);
            selection = selection.united(rect);
        }
    }
    const double dpr = qMin(image->devicePixelRatioF(), 4096.0 / qMax(size.width(), size.height()));
    QPixmap pixmap = QPixmap::fromImage(annotated.scaled(size * dpr, Qt::KeepAspectRatio, Qt::SmoothTransformation));
    pixmap.setDevicePixelRatio(dpr);
    image->setPixmap(pixmap);
    image->resize(size);
    if (centerMatch && !selection.isEmpty())
        scroller->ensureVisible(qRound(selection.center().x()*size.width()/raster.width()),
                                qRound(selection.center().y()*size.height()/raster.height()), 48, 36);
}

bool PdfPane::eventFilter(QObject *object, QEvent *event)
{
    if (object == scroller->viewport() && event->type() == QEvent::Resize && fitToPage)
        draw(false);
    return QWidget::eventFilter(object, event);
}
}
