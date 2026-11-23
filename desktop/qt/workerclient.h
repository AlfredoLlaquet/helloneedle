// SPDX-License-Identifier: GPL-3.0-or-later
#pragma once
#include <QObject>
#include <QJsonObject>
#include <functional>

class QProcess;

namespace hn {
QString findTool(const QString &name);
class WorkerClient final : public QObject {
public:
    using Event = std::function<void(const QJsonObject &)>;
    using Finished = std::function<void(const QString &)>;
    explicit WorkerClient(QObject *parent = nullptr);
    ~WorkerClient() override;
    void start(const QString &command, const QJsonObject &request, Event event, Finished finished);
    void stop();

private:
    void consume(bool final);
    void finish(const QString &error);
    QProcess *process = nullptr;
#ifdef Q_OS_WIN
    void *processTree = nullptr;
#endif
    QByteArray pending;
    Event onEvent;
    Finished onFinished;
};
}
