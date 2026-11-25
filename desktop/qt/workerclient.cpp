// SPDX-License-Identifier: GPL-3.0-or-later
#include "workerclient.h"
#include "language.h"
#include "runtime.h"
#include <QElapsedTimer>
#include <QJsonDocument>
#include <QPointer>
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
    scannedPrefix = 0;
    readQueued = false;
    exited = false;
    exitError.clear();
    onEvent = {};
    onFinished = {};
}

void WorkerClient::finish(const QString &error)
{
    const auto callback = onFinished;
    stop();
    if (callback) callback(error);
}

void WorkerClient::scheduleRead()
{
    if (!process || readQueued) return;
    readQueued = true;
    QTimer::singleShot(0, this, [this, job = QPointer<QProcess>(process)] {
        if (!job || process != job.data()) return;
        readQueued = false;
        consume();
    });
}

void WorkerClient::consume()
{
    auto *job = process;
    pending += job->readAllStandardOutput();
    // Each event is a single frame. Bound a malformed or unexpectedly large
    // response without retaining a growing pipe between searches.
    constexpr qsizetype limit = 64 * 1024 * 1024;
    QElapsedTimer slice;
    slice.start();
    qsizetype pos = 0;
    while (pos < pending.size()) {
        qsizetype end = pending.indexOf('\n', qMax(pos, scannedPrefix));
        if (end < 0 && !exited && pending.size() - pos <= limit) {
            scannedPrefix = pending.size() - pos;
            break;
        }
        scannedPrefix = 0;
        if (end < 0) end = pending.size();
        if (end - pos > limit) { finish(ui("The worker response is too large.")); return; }
        const auto line = pending.mid(pos, end - pos).trimmed();
        pos = end + 1;
        if (line.isEmpty()) continue;
        const auto json = QJsonDocument::fromJson(line);
        if (!json.isObject()) { finish(ui("The worker returned an unreadable response.")); return; }
        if (onEvent) onEvent(json.object());
        if (process != job) return;
        // A burst of hits from several readers must leave the GUI time to handle
        // navigation, previews and Cancel before inserting the next group of rows.
        if (slice.elapsed() >= 6 && pos < pending.size()) {
            pending.remove(0, pos);
            scheduleRead();
            return;
        }
    }
    pending.remove(0, pos);
    if (exited) {
        const QString error = exitError;
        finish(error);
    }
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
        if (process == job) scheduleRead();
    });
    connect(job, &QProcess::errorOccurred, this, [this, job](QProcess::ProcessError error) {
        if (process == job && error == QProcess::FailedToStart)
            finish(ui("The search worker could not be started."));
    });
    connect(job, &QProcess::finished, this, [this, job](int code, QProcess::ExitStatus status) {
        if (process != job) return;
        exited = true;
        if (status != QProcess::NormalExit || code != 0) exitError = ui("The search worker stopped unexpectedly.");
        scheduleRead();
    });
    job->start(program, {command});
}
}
