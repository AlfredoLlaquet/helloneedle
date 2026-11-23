// SPDX-License-Identifier: GPL-3.0-or-later
#pragma once

#include <QString>
#include <QStringList>

class QJsonObject;

namespace hn {
void setLanguage(const QString &localeName);
QString currentLanguage();
QString systemLanguage(const QStringList &uiLanguages);
QString ui(const char *english);
QString locationLabel(const QJsonObject &location, const QString &fallback);
QString workerError(const QJsonObject &event);
}
