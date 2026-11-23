// SPDX-License-Identifier: GPL-3.0-or-later
#include "application.h"
#include "language.h"

#include <QApplication>
#include <QIcon>
#include <QLocale>
#include <QPainter>
#include <QPainterPath>
#include <QSettings>

namespace {
QIcon needleIcon()
{
    QIcon icon;
    for (const int size : {24, 32, 48, 64, 128, 256}) {
        QPixmap tile(size, size);
        tile.fill(Qt::transparent);
        QPainter p(&tile);
        p.setRenderHint(QPainter::Antialiasing);
        p.scale(size / 64.0, size / 64.0);
        p.setPen(Qt::NoPen);
        p.setBrush(QColor(19, 75, 78));
        p.drawRoundedRect(QRectF(1, 1, 62, 62), 14, 14);
        QPainterPath needle;
        needle.moveTo(12, 52);
        needle.quadTo(24, 33, 42, 14);
        needle.cubicTo(49, 7, 58, 14, 51, 22);
        needle.quadTo(32, 41, 12, 52);
        needle.closeSubpath();
        p.setBrush(QColor(252, 241, 210));
        p.drawPath(needle);
        p.setPen(QPen(QColor(19, 75, 78), 3.3, Qt::SolidLine, Qt::RoundCap));
        p.drawLine(QPointF(44, 21), QPointF(48, 17));
        p.end();
        icon.addPixmap(tile);
    }
    return icon;
}
}
namespace hn {
void initializeApplication(QApplication &app)
{
    app.setOrganizationName(QStringLiteral("Hello Needle"));
    app.setOrganizationDomain(QStringLiteral("thecrumbs.org"));
    app.setApplicationName(QStringLiteral("helloneedle"));
    app.setApplicationDisplayName(QStringLiteral("Hello Needle"));
    app.setApplicationVersion(QStringLiteral(HN_VERSION));
    app.setDesktopFileName(QStringLiteral("org.thecrumbs.HelloNeedle"));
    app.setWindowIcon(needleIcon());
    QSettings settings;
    const QString language = settings.value(QStringLiteral("interface/language")).toString();
    setLanguage(language.isEmpty() ? systemLanguage(QLocale::system().uiLanguages()) : language);
}
}
