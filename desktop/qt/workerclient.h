// SPDX-License-Identifier: GPL-3.0-or-later
#pragma once
#include <QObject>
#include <QSet>
#include <QJsonObject>
#include <functional>

class QProcess;

namespace hn {
class SearchPump;
class WorkerClient final : public QObject {
public:
    using Event = std::function<void(const QJsonObject &)>;
    using Finished = std::function<void(const QString &)>;
    explicit WorkerClient(QObject *parent = nullptr);
    ~WorkerClient() override;
    void start(const QString &command, const QJsonObject &request, Event event, Finished finished);
    void stop();

private:
    void scheduleRead();
    void consume();
    void searchBytes(SearchPump *owner, const QByteArray &data, bool final);
    void finish(const QString &error);
    QProcess *process = nullptr;
    SearchPump *searchPump = nullptr;
    QSet<SearchPump *> pumps;
#ifdef Q_OS_WIN
    void *processTree = nullptr;
#endif
    QByteArray pending;
    qsizetype scannedPrefix = 0;
    QString exitError;
    bool readQueued = false;
    bool exited = false;
    Event onEvent;
    Finished onFinished;
};
}
