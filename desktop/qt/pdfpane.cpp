// SPDX-License-Identifier: GPL-3.0-or-later
#include "pdfpane.h"
#include "language.h"
#include "runtime.h"
#include <QEvent>
#include <QFile>
#include <QHBoxLayout>
#include <QJsonArray>
#include <QLabel>
#include <QPainter>
#include <QProcess>
#include <QScrollArea>
#include <QScrollBar>
#include <QTimer>
#include <QToolButton>
#include <QVBoxLayout>
#include <algorithm>
#include <cmath>
#ifdef Q_OS_UNIX
#include <fcntl.h>
#include <unistd.h>
#endif

namespace hn {
namespace {
// A single cropped render follows the viewport. A small whole-page image
// remains available while Poppler draws new detail after scrolling or zoom.
constexpr int overviewPixels = 1800;
void stopRender(QProcess *&process, QObject *receiver)
{
    auto *old = process;
    process = nullptr;
    if (!old) return;
    old->disconnect(receiver);
    if (old->state() != QProcess::NotRunning) {
        QObject::connect(old, &QProcess::finished, old, &QObject::deleteLater);
        old->kill();
    } else old->deleteLater();
}
}

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
    detail = new QLabel(image);
    detail->setObjectName("pdfDetailRaster");
    detail->setScaledContents(true);
    detail->setAttribute(Qt::WA_TransparentForMouseEvents);
    detail->hide();
    detailTimer = new QTimer(this);
    detailTimer->setSingleShot(true);
    connect(detailTimer, &QTimer::timeout, this, &PdfPane::renderDetail);
    connect(scroller->horizontalScrollBar(), &QScrollBar::valueChanged, this, [this] { scheduleDetail(); });
    connect(scroller->verticalScrollBar(), &QScrollBar::valueChanged, this, [this] { scheduleDetail(); });
    scroller->setWidget(image);
    layout->addWidget(scroller);
    connect(previous, &QToolButton::clicked, this, [this]{ goTo(page - 1); });
    connect(next, &QToolButton::clicked, this, [this]{ goTo(page + 1); });
    connect(fit, &QToolButton::clicked, this, [this]{ fitToPage = true; draw(true); });
    auto changeZoom = [this](double multiplier) {
        if (raster.isNull()) return;
        if (fitToPage) zoom = double(image->width()) / raster.width();
        // Both Qt widget geometry and Poppler crop coordinates must remain
        // representable. Pixel allocations depend on the viewport, not zoom.
        const double longest = qMax(raster.width(), raster.height());
        zoom = std::clamp(zoom * multiplier, 1. / longest, (QWIDGETSIZE_MAX - 1.) / longest);
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
    detailTimer->stop();
    stopRender(job, this);
    stopRender(detailJob, this);
    detail->clear();
    detail->hide();
}

void PdfPane::clear()
{
    cancel();
    pendingCenter.reset();
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
    const int selected = qMax(1, location.value("page").toInt(1));
    pages = qMax(selected, location.value("pageCount").toInt(selected));
    const QString identity = match.value("pathBytes").toString(match.value("path").toString());
    if (identity != zoomFile) {
        zoomFile = identity;
        zoom = 1;
        fitToPage = true;
    }
    goTo(selected);
}

void PdfPane::goTo(int target)
{
    if (target < 1 || target > pages) return;
    cancel();
    pendingCenter.reset();
    page = target;
    raster = {};
    image->clear();
    image->setText(ui("Loading page…"));
    image->resize(scroller->viewport()->size());
    previous->setEnabled(page > 1);
    next->setEnabled(page < pages);
    pageLabel->setText(ui("Page %1 of %2").arg(page).arg(pages));
    startRaster(overviewPixels, {}, {}, 1.);
}

void PdfPane::startRaster(int longSide, const QRect &crop, const QRect &logical, double pixelRatio)
{
    const bool cropped = !crop.isEmpty();
    const auto program = findTool("pdftoppm");
    if (program.isEmpty()) {
        image->setText(ui("PDF page rendering requires Poppler (pdftoppm)."));
        return;
    }
    auto *p = new QProcess(this);
    (cropped ? detailJob : job) = p;
    configurePdfRenderer(*p, program);
    p->setStandardErrorFile(QProcess::nullDevice());
    const QString file = hit.value("path").toString();
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
    connect(p, &QProcess::errorOccurred, this, [this, p, cropped](QProcess::ProcessError err) {
        auto &active = cropped ? detailJob : job;
        if (p == active && err == QProcess::FailedToStart) {
            if (!cropped) image->setText(ui("The PDF page could not be rendered."));
            stopRender(active, this);
        }
    });
    connect(p, &QProcess::finished, this, [this, p, crop, logical, cropped, pixelRatio](int code, QProcess::ExitStatus status) {
        auto &active = cropped ? detailJob : job;
        if (p != active) return;
        QImage result;
        if (code == 0 && status == QProcess::NormalExit)
            result = QImage::fromData(p->readAllStandardOutput(), "PNG");
        stopRender(active, this);
        if (cropped) {
            if (result.isNull()) return;
            annotate(result, QSizeF(image->size()) * pixelRatio, crop.topLeft());
            detail->setGeometry(logical);
            detail->setPixmap(QPixmap::fromImage(result));
            detail->show();
        } else {
            raster = result;
            if (raster.isNull()) image->setText(ui("The PDF page could not be rendered."));
            else draw(true);
        }
    });
    QStringList options{"-f", QString::number(page), "-l", QString::number(page),
                        "-singlefile", "-scale-to", QString::number(longSide)};
    if (cropped) options << "-x" << QString::number(crop.x()) << "-y" << QString::number(crop.y())
                         << "-W" << QString::number(crop.width()) << "-H" << QString::number(crop.height());
    options << "-png" << argument;
    p->start(program, options, QIODevice::ReadOnly);
}

QRectF PdfPane::annotate(QImage &canvas, const QSizeF &fullSize, const QPointF &origin) const
{
    double width = spot.value("width").toDouble(), height = spot.value("height").toDouble();
    const double ratio = double(raster.width()) / raster.height();
    if (width > 0 && height > 0 && std::abs(ratio - height/width) < std::abs(ratio - width/height))
        std::swap(width, height);
    QRectF selection;
    if (page == spot.value("page").toInt() && width > 0 && height > 0 && hit.value("kind") != "filename") {
        const auto begin = hit.value("blockStart").toInteger(hit.value("start").toInteger());
        const auto end = begin + hit.value("blockLength").toInteger(hit.value("length").toInteger());
        QPainter painter(&canvas);
        painter.translate(-origin);
        painter.setPen(QPen(QColor(130, 66, 0), 2));
        painter.setBrush(QColor(255, 198, 49, 105));
        auto highlight = [&](const QJsonObject &word) {
            const double sx = fullSize.width() / width, sy = fullSize.height() / height;
            const QRectF rect(word.value("x").toDouble()*sx, word.value("y").toDouble()*sy,
                              word.value("w").toDouble()*sx, word.value("h").toDouble()*sy);
            painter.drawRect(rect);
            selection = selection.united(rect);
        };
        if (const auto full = spot.value("fullMatchBox").toObject(); !full.isEmpty()) {
            highlight(full);
        } else for (const auto &entry : spot.value("words").toArray()) {
            const QJsonObject word = entry.toObject();
            const auto start = word.value("start").toInteger();
            if (start < end && start + word.value("length").toInteger() > begin) highlight(word);
        }
    }
    return selection;
}

void PdfPane::draw(bool centerMatch)
{
    if (raster.isNull()) return;
    const QSize available = scroller->viewport()->size() - QSize(8, 8);
    const QSize size = fitToPage ? raster.size().scaled(available.expandedTo({1,1}), Qt::KeepAspectRatio)
                                 : (raster.size() * zoom).expandedTo({1,1});
    if (size.isEmpty()) return;
    stopRender(detailJob, this);
    detail->hide();
    QImage annotated = raster;
    const QRectF selection = annotate(annotated, raster.size(), {});
    const double dpr = qMin(image->devicePixelRatioF(), 4096.0 / qMax(size.width(), size.height()));
    QPixmap pixmap = QPixmap::fromImage(annotated.scaled(size * dpr, Qt::KeepAspectRatio, Qt::SmoothTransformation));
    pixmap.setDevicePixelRatio(dpr);
    image->setPixmap(pixmap);
    image->resize(size);
    scheduleDetail();
    if (centerMatch) {
        pendingCenter.reset();
        if (!fitToPage && !selection.isEmpty()) {
            pendingCenter = QPoint(qRound(selection.center().x() * size.width() / raster.width()),
                                   qRound(selection.center().y() * size.height() / raster.height()));
            // Let Qt settle both scroll ranges after the raster changes size.
            QTimer::singleShot(0, this, &PdfPane::centerPendingMatch);
        }
    }
}

void PdfPane::scheduleDetail()
{
    if (!raster.isNull() && scroller->isVisible()) detailTimer->start(35);
}

void PdfPane::renderDetail()
{
    if (raster.isNull() || !isVisible()) return;
    const auto *viewport = scroller->viewport();
    const QRect visible = image->rect().intersected(QRect(image->mapFrom(viewport, QPoint()), viewport->size()));
    if (visible.isEmpty()) return;
    // Bound one tile to at most 4096 by 4096 physical pixels, regardless of
    // the document's dimensions. A large zoom only moves the crop coordinates.
    const double deviceScale = qMin(image->devicePixelRatioF(),
        4096. / qMax(visible.width(), visible.height()));
    const int longSide = qRound(qMax(image->width(), image->height()) * deviceScale);
    if (longSide <= qMax(raster.width(), raster.height())) return;
    const int x = qFloor(visible.x() * deviceScale);
    const int y = qFloor(visible.y() * deviceScale);
    const QRect crop(x, y, qMax(1, qCeil((visible.x()+visible.width())*deviceScale)-x),
                     qMax(1, qCeil((visible.y()+visible.height())*deviceScale)-y));
    stopRender(detailJob, this);
    startRaster(longSide, crop, visible, deviceScale);
}

void PdfPane::centerPendingMatch()
{
    if (!pendingCenter || !scroller->isVisible()) return;
    auto *viewport = scroller->viewport();
    const QPoint offset = image->mapTo(viewport, *pendingCenter) - viewport->rect().center();
    pendingCenter.reset();
    auto *horizontal = scroller->horizontalScrollBar();
    auto *vertical = scroller->verticalScrollBar();
    horizontal->setValue(horizontal->value() + offset.x());
    vertical->setValue(vertical->value() + offset.y());
}

bool PdfPane::eventFilter(QObject *object, QEvent *event)
{
    if (object == scroller->viewport() && event->type() == QEvent::Resize) {
        if (fitToPage) draw(false);
        else scheduleDetail();
    }
    if (object == scroller->viewport() && event->type() == QEvent::Show) scheduleDetail();
    if (object == scroller->viewport() && event->type() == QEvent::Show && pendingCenter)
        QTimer::singleShot(0, this, &PdfPane::centerPendingMatch);
    return QWidget::eventFilter(object, event);
}
}
