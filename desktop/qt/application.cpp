// SPDX-License-Identifier: GPL-3.0-or-later
#include "application.h"
#include "language.h"

#include <QApplication>
#include <QIcon>
#include <QImage>
#include <QLocale>
#include <QPixmap>
#include <QSettings>

namespace {
QIcon cowIcon()
{
    const QImage artwork(QStringLiteral(":/art/helloneedle.png"));
    QIcon icon;
    // Native title bars, task switchers and high-density displays request
    // different physical sizes. Sample each from the original transparent art.
    for (const int size : {16, 20, 24, 32, 40, 48, 64, 96, 128, 256, 512})
        icon.addPixmap(QPixmap::fromImage(artwork.scaled(size, size, Qt::KeepAspectRatio, Qt::SmoothTransformation)));
    icon.addPixmap(QPixmap::fromImage(artwork));
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
    app.setWindowIcon(cowIcon());
    QSettings settings;
    const QString language = settings.value(QStringLiteral("interface/language")).toString();
    setLanguage(language.isEmpty() ? systemLanguage(QLocale::system().uiLanguages()) : language);
}
}
