// SPDX-License-Identifier: GPL-3.0-or-later
#include "gui_icon.h"

#include <QDir>
#include <QIcon>
#include <QImage>
#include <QPainter>
#include <algorithm>

namespace {
int colorDifference(QRgb a, QRgb b)
{
    return std::max({std::abs(qRed(a) - qRed(b)), std::abs(qGreen(a) - qGreen(b)),
                     std::abs(qBlue(a) - qBlue(b)), std::abs(qAlpha(a) - qAlpha(b))});
}

QImage onBackground(const QImage &icon, QColor background)
{
    QImage applied(icon.size(), QImage::Format_RGB32);
    applied.fill(background);
    QPainter painter(&applied);
    painter.drawImage(0, 0, icon);
    return applied;
}
}

namespace guiqa {
QString verifyApplicationIcon(const QIcon &icon, QJsonObject &evidence)
{
    const QString masterPath = qEnvironmentVariable("HN_QA_MASTER_ICON");
    const QImage master(masterPath), resource(":/art/helloneedle.png");
    if (master.isNull() || master.width() < 256 || master.width() != master.height())
        return "Missing square master artwork for Hello Needle";
    if (resource.isNull() || resource.convertToFormat(QImage::Format_RGBA8888) !=
                             master.convertToFormat(QImage::Format_RGBA8888))
        return "Embedded artwork differs from the source cow icon";
    if (icon.isNull())
        return "Window has no application icon";

    constexpr int panelWidth = 220, cellHeight = 100;
    QImage grid(panelWidth * 2, cellHeight * 3, QImage::Format_RGB32);
    QPainter painter(&grid);
    QFont font = painter.font();
    font.setPixelSize(13);
    painter.setFont(font);
    const int sizes[] = {16, 32, 64};
    const QColor backgrounds[] = {QColor("#f4f4f4"), QColor("#25282b")};
    QString failure;
    for (int row = 0; row < 3; ++row) {
        const int s = sizes[row];
        const auto pixmap = icon.pixmap(QSize(s, s), 1.0, QIcon::Normal, QIcon::Off);
        if (pixmap.isNull() || pixmap.size() != QSize(s, s))
            return QString("No native application icon at %1 px").arg(s);
        const QImage actual = pixmap.toImage().convertToFormat(QImage::Format_ARGB32_Premultiplied);
        const QImage expected = master.scaled(s, s, Qt::KeepAspectRatio, Qt::SmoothTransformation)
                                      .convertToFormat(QImage::Format_ARGB32_Premultiplied);
        // Premultiplied pixels exclude irrelevant RGB values outside the face.
        // The small tolerance permits native format conversion and rounding.
        int changed = 0;
        double totalDelta = 0;
        for (int y = 0; y < s; ++y)
            for (int x = 0; x < s; ++x) {
                const int delta = colorDifference(actual.pixel(x, y), expected.pixel(x, y));
                totalDelta += delta;
                changed += delta > 12;
            }
        const double mean = totalDelta / (s*s);
        QJsonObject stats{{"meanMaxChannelDifference", mean}, {"overTolerance", changed}};
        if (mean > 3 || changed > s*s/20)
            failure += QString("Window icon does not match the cow at %1 px. ").arg(s);

        for (int col = 0; col < 2; ++col) {
            const QColor bg = backgrounds[col];
            const auto shown = onBackground(actual, bg);
            int distinguishable = 0;
            for (int y = 0; y < s; ++y)
                for (int x = 0; x < s; ++x)
                    distinguishable += colorDifference(shown.pixel(x, y), bg.rgb()) >= 32;
            const double coverage = double(distinguishable) / (s*s);
            stats[col == 0 ? "lightVisibleFraction" : "darkVisibleFraction"] = coverage;
            if (coverage < 0.20)
                failure += QString("Cow nearly disappears on the %1 background at %2 px. ")
                               .arg(col == 0 ? "light" : "dark").arg(s);
            const int x = col*panelWidth, y = row*cellHeight;
            painter.fillRect(QRect(x,y,panelWidth,cellHeight), bg);
            painter.setPen(col == 0 ? Qt::black : Qt::white);
            painter.drawText(QPoint(x+10,y+18), QString("%1 px").arg(s));
            painter.drawImage(x + (panelWidth-s)/2, y+23 + (68-s)/2, shown);
        }
        evidence[QString::number(s)] = stats;
    }
    painter.end();
    const auto path = QDir(qEnvironmentVariable("HN_QA_OUTPUT")).filePath("icon-grid.png");
    if (!grid.save(path))
        failure += "Could not save the light/dark icon grid.";
    return failure;
}
}
