// SPDX-License-Identifier: GPL-3.0-or-later
#pragma once

class QJsonObject;
class QWidget;

namespace hn {
void showPerformanceSettings(QWidget *parent);
void applyPerformanceSettings(QJsonObject &searchRequest);
}
