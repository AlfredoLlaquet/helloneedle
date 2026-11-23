// SPDX-License-Identifier: GPL-3.0-or-later
#include "workerclient.h"
#include "language.h"
#include "runtime.h"
#include <QJsonDocument>
#include <QProcess>
#include <QTimer>
#ifdef Q_OS_UNIX
#include <signal.h>
#include <unistd.h>
#elif defined(Q_OS_WIN)
#include <qt_windows.h>
#endif

namespace hn {
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
#ifdef Q_OS_UNIX
            const pid_t pid = static_cast<pid_t>(old->processId());
            if (pid > 0 && ::getpgid(pid) == pid) ::kill(-pid, SIGKILL);
#endif
            old->kill();
        } else old->deleteLater();
    }
#ifdef Q_OS_WIN
    if (processTree) { CloseHandle(processTree); processTree = nullptr; }
#endif
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
#ifdef Q_OS_UNIX
    // A query may start a PDF or Word decoder. Stop the full group when its
    // owning search is canceled, even while the decoder is still reading.
    job->setChildProcessModifier([] { ::setpgid(0, 0); });
#endif
    connect(job, &QProcess::started, this, [this, job, request] {
#ifdef Q_OS_WIN
        // Assign before sending the request, so descendants enter the job
        // before processing their first document.
        auto group = CreateJobObjectW(nullptr, nullptr);
        if (group) {
            JOBOBJECT_EXTENDED_LIMIT_INFORMATION limits{};
            limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            auto child = OpenProcess(PROCESS_SET_QUOTA | PROCESS_TERMINATE, FALSE, DWORD(job->processId()));
            if (child && SetInformationJobObject(group, JobObjectExtendedLimitInformation, &limits, sizeof(limits))
                && AssignProcessToJobObject(group, child)) processTree = group;
            else CloseHandle(group);
            if (child) CloseHandle(child);
        }
#endif
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
