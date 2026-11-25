// SPDX-License-Identifier: GPL-3.0-or-later
#pragma once
#include <QJsonObject>
#include <QTemporaryDir>

class QAbstractButton;

namespace guiqa {
// Synchronize two batches of results at the real desktop/worker boundary. This
// lets the GUI test interact during a search without depending on disk speed.
class SearchStream {
public:
    QTemporaryDir folder;
    bool prepare(const QByteArray &first, const QByteArray &rest) const;
    void start(QAbstractButton &button) const;
    bool resume() const;
    QJsonObject request() const;
};

// The GUI test executable also serves as the controllable search child.
// Preview requests still launch the installed Rust worker.
int replaySearchStream();
}
