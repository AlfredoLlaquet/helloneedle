// SPDX-License-Identifier: GPL-3.0-or-later
#pragma once

#include <QImage>
#include <QJsonObject>
#include <QRect>
#include <QWindow>

class QWidget;

namespace guiqa {
struct WindowFrame {
    QImage image;
    QRect clientPixels;
};

bool matchesWindowClient(const QImage &candidate, const QRect &area,
                         const QImage &clientReference, QJsonObject &metrics);
bool hasWindowsCaption(const QImage &frame, const QRect &clientPixels, QJsonObject &metrics);
void captureFirstWindow(QWidget &window, QJsonObject &evidence);

#ifdef Q_OS_WIN
WindowFrame printWindowsFrame(WId id);
#endif
}
