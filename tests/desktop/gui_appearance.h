// SPDX-License-Identifier: GPL-3.0-or-later
#pragma once

#include <QJsonObject>
#include <QString>
#include <QRect>

class QApplication;
class QLineEdit;
class QWidget;

namespace guiqa {

QString setAppearance(QApplication &app, const QString &requested);
QJsonObject platformDetails(const QString &appearanceSource);
QString inspectLayout(QWidget &window);
QString inspectColors(QWidget &window, QJsonObject &colors);
QString inspectMenuInk(QWidget &menu, const QRect &action, QJsonObject &metrics);
QString inspectRenderedField(QLineEdit &edit, QJsonObject &metrics);
QString saveView(QWidget &widget, const QString &name);
bool makeReviewSheet(const QString &root);
bool hasExpectedPlatform();

}
