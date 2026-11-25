// SPDX-License-Identifier: GPL-3.0-or-later
#pragma once
#include <QImage>
#include <QJsonObject>
#include <QWidget>
#include <optional>

class QProcess;
class QLabel;
class QScrollArea;
class QToolButton;
class QTimer;

namespace hn {
class PdfPane : public QWidget {
public:
    explicit PdfPane(QWidget *parent = nullptr);
    void open(const QJsonObject &match, const QJsonObject &location);
    void clear();
    void retranslate();
    int currentPage() const { return page; }
protected:
    bool eventFilter(QObject *object, QEvent *event) override;
private:
    void goTo(int page);
    void draw(bool centerMatch);
    void scheduleDetail();
    void renderDetail();
    void startRaster(int longSide, const QRect &crop, const QRect &logical, double pixelRatio);
    QRectF annotate(QImage &canvas, const QSizeF &fullSize, const QPointF &origin) const;
    void centerPendingMatch();
    void cancel();
    QString zoomFile;
    std::optional<QPoint> pendingCenter;
    QJsonObject hit;
    QJsonObject spot;
    QImage raster;
    QProcess *job = nullptr;
    QProcess *detailJob = nullptr;
    QTimer *detailTimer;
    QLabel *image;
    QLabel *detail;
    QLabel *pageLabel;
    QScrollArea *scroller;
    QToolButton *previous;
    QToolButton *next;
    QToolButton *zoomIn;
    QToolButton *zoomOut;
    QToolButton *fit;
    double zoom = 1;
    bool fitToPage = true;
    int page = 0;
    int pages = 0;
};
}
