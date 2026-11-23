// SPDX-License-Identifier: GPL-3.0-or-later
#include "workerclient.h"
#include "language.h"
#include <QCoreApplication>
#include <QDir>
#include <QFileInfo>
#include <QJsonDocument>
#include <QProcess>
#include <QStandardPaths>
#include <QTimer>

namespace hn {
QString findTool(const QString &name)
{
    QString file = name;
#ifdef Q_OS_WIN
    file += QStringLiteral(".exe");
#endif
    if (name == QStringLiteral("helloneedle-worker")) {
        for (const auto key : {"HELLONEEDLE_WORKER", "HELLO_NEEDLE_WORKER"}) {
            const auto overridePath = qEnvironmentVariable(key);
            if (!overridePath.isEmpty()) return overridePath;
        }
    }
    const QDir app(QCoreApplication::applicationDirPath());
    QStringList directories{app.path(), app.filePath("../libexec/helloneedle"), app.filePath("../libexec")};
#ifdef Q_OS_MACOS
    directories << QStringLiteral("/opt/homebrew/bin") << QStringLiteral("/usr/local/bin");
#endif
    for (const auto &directory : directories) {
        const QFileInfo candidate(QDir(directory).filePath(file));
        if (candidate.isFile() && candidate.isExecutable()) return candidate.absoluteFilePath();
    }
    return QStandardPaths::findExecutable(file);
}

WorkerClient::WorkerClient(QObject *parent) : QObject(parent) {}
WorkerClient::~WorkerClient() { stop(); }

void WorkerClient::stop()
{
    if (process) {
        auto *old = process;
        process = nullptr;
        old->disconnect(this);
        if (old->state() != QProcess::NotRunning) {
            connect(old, &QProcess::finished, old, &QObject::deleteLater);
            old->kill();
        } else old->deleteLater();
    }
    pending.clear();
    onEvent = {};
    onFinished = {};
}

void WorkerClient::finish(const QString &error)
{
    const auto callback = onFinished;
    stop();
    if (callback) callback(error);
}

void WorkerClient::consume(bool final)
{
    if (!process) return;
    pending += process->readAllStandardOutput();
    // Each event is a single frame. Bound a malformed or unexpectedly large
    // response without retaining a growing pipe between searches.
    constexpr qsizetype limit = 64 * 1024 * 1024;
    qsizetype pos = 0;
    while (process && pos < pending.size()) {
        qsizetype end = pending.indexOf('\n', pos);
        if (end < 0 && !final && pending.size() - pos <= limit) break;
        if (end < 0) end = pending.size();
        if (end - pos > limit) { finish(ui("The worker response is too large.")); return; }
        const auto line = pending.mid(pos, end - pos).trimmed();
        pos = end + 1;
        if (line.isEmpty()) continue;
        const auto json = QJsonDocument::fromJson(line);
        if (!json.isObject()) { finish(ui("The worker returned an unreadable response.")); return; }
        if (onEvent) onEvent(json.object());
    }
    pending.remove(0, pos);
}

void WorkerClient::start(const QString &command, const QJsonObject &request, Event event, Finished finished)
{
    stop();
    onEvent = std::move(event);
    onFinished = std::move(finished);
    const QString program = findTool(QStringLiteral("helloneedle-worker"));
    if (program.isEmpty()) {
        finish(ui("The search worker could not be started."));
        return;
    }
    process = new QProcess(this);
    auto *job = process;
    job->setStandardErrorFile(QProcess::nullDevice());
    connect(job, &QProcess::started, this, [job, request] {
        job->write(QJsonDocument(request).toJson(QJsonDocument::Compact) + '\n');
        job->closeWriteChannel();
    });
    connect(job, &QProcess::readyReadStandardOutput, this, [this, job] {
        if (process == job) consume(false);
    });
    connect(job, &QProcess::errorOccurred, this, [this, job](QProcess::ProcessError error) {
        if (process == job && error == QProcess::FailedToStart)
            finish(ui("The search worker could not be started."));
    });
    connect(job, &QProcess::finished, this, [this, job](int code, QProcess::ExitStatus status) {
        if (process != job) return;
        consume(true);
        if (process == job) finish(status == QProcess::NormalExit && code == 0
                          ? QString() : ui("The search worker stopped unexpectedly."));
    });
    job->start(program, {command});
}
}

