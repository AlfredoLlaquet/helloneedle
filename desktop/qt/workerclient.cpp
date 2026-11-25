// SPDX-License-Identifier: GPL-3.0-or-later
#include "workerclient.h"
#include "language.h"
#include "runtime.h"
#include "searchpump.h"
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
WorkerClient::~WorkerClient()
{
    stop();
    // A replacement search may briefly overlap a canceled pipe pump. None of
    // those threads may post a delivery after its owning client is destroyed.
    for (auto *pump : pumps) { pump->cancel(); pump->wait(); delete pump; }
}

void WorkerClient::stop()
{
    if (searchPump) { searchPump->cancel(); searchPump = nullptr; }
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
        const auto callback = onEvent;
        if (callback) callback(json.object());
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

void WorkerClient::searchBytes(SearchPump *owner, const QByteArray &data, bool final)
{
    if (searchPump != owner) return;
    constexpr qsizetype frameLimit = 64 * 1024 * 1024;
    pending += data;
    qsizetype pos = 0;
    while (pos < pending.size()) {
        auto end = pending.indexOf('\n', qMax(scannedPrefix, pos));
        if (end < 0 && !final) {
            if (pending.size() - pos > frameLimit) { finish(ui("The worker response is too large.")); return; }
            scannedPrefix = pending.size() - pos;
            break;
        }
        if (end < 0) end = pending.size();
        if (end - pos > frameLimit) { finish(ui("The worker response is too large.")); return; }
        scannedPrefix = 0;
        const auto frame = pending.mid(pos, end - pos).trimmed();
        pos = end + 1;
        if (frame.isEmpty()) continue;
        const auto message = QJsonDocument::fromJson(frame);
        if (!message.isObject()) { finish(ui("The worker returned an unreadable response.")); return; }
        const auto callback = onEvent;
        if (callback) callback(message.object());
        if (searchPump != owner) return;
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
    constexpr qsizetype requestLimit = 1024 * 1024;
    // UTF-8 JSON needs at least one byte per UTF-16 unit. Avoid making a
    // second large allocation for input that cannot fit the worker protocol.
    const auto querySize = request.value("query").toString().size();
    if (querySize > requestLimit || request.value("pathPattern").toString().size() > requestLimit
        || request.value("folder").toString().size() > requestLimit) {
        finish(ui("Search input is too long."));
        return;
    }
    const auto encoded = QJsonDocument(request).toJson(QJsonDocument::Compact) + '\n';
    if (encoded.size() > requestLimit) { finish(ui("Search input is too long.")); return; }
    if (command == QStringLiteral("search")) {
        auto *pump = new SearchPump(program, encoded, this, [this](SearchPump *owner, const QByteArray &bytes) {
            searchBytes(owner, bytes, false);
        });
        pumps.insert(pump);
        searchPump = pump;
        connect(pump, &QThread::finished, this, [this, pump] {
            if (pump == searchPump) {
                searchBytes(pump, {}, true);
                if (pump == searchPump) {
                    QString error;
                    if (pump->outcome() == SearchPump::Outcome::StartFailed)
                        error = ui("The search worker could not be started.");
                    else if (pump->outcome() == SearchPump::Outcome::WorkerFailed)
                        error = ui("The search worker stopped unexpectedly.");
                    finish(error);
                }
            }
            pumps.remove(pump);
            pump->deleteLater();
        }, Qt::QueuedConnection);
        pump->start();
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
    connect(job, &QProcess::started, this, [this, job, encoded] {
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
        job->write(encoded);
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
