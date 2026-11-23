// SPDX-License-Identifier: GPL-3.0-or-later
#pragma once
#include <QString>

class QProcess;

namespace hn {
QString findTool(const QString &name);
void configurePdfRenderer(QProcess &process, const QString &program);
}
