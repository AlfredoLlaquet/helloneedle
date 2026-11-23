// SPDX-License-Identifier: GPL-3.0-or-later
#pragma once

#include <QJsonObject>
#include <QString>

class QIcon;

namespace guiqa {
QString verifyApplicationIcon(const QIcon &icon, QJsonObject &evidence);
}
