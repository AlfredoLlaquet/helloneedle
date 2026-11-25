// SPDX-License-Identifier: GPL-3.0-or-later
#include "searchpump.h"
#include <QPointer>
#include <QProcess>
#ifdef Q_OS_UNIX
#include <signal.h>
#include <unistd.h>
#elif defined(Q_OS_WIN)
#include <qt_windows.h>
#endif

namespace hn {
SearchPump::SearchPump(QString name, QByteArray bytes, QObject *target, Delivery callback)
    : executable(std::move(name)), request(std::move(bytes)), receiver(target), deliver(std::move(callback)) {}

void SearchPump::cancel() { canceled = true; }

void SearchPump::run()
{
    QProcess job;
    job.setStandardErrorFile(QProcess::nullDevice());
#ifdef Q_OS_UNIX
    job.setChildProcessModifier([] { ::setpgid(0, 0); });
#endif
    job.start(executable, {QStringLiteral("search")});
    if (!job.waitForStarted()) { ending = Outcome::StartFailed; return; }
#ifdef Q_OS_WIN
    HANDLE group = CreateJobObjectW(nullptr, nullptr);
    if (group) {
        JOBOBJECT_EXTENDED_LIMIT_INFORMATION info{};
        info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        auto process = OpenProcess(PROCESS_SET_QUOTA | PROCESS_TERMINATE, FALSE, DWORD(job.processId()));
        const bool assigned = process && SetInformationJobObject(group, JobObjectExtendedLimitInformation, &info, sizeof(info))
            && AssignProcessToJobObject(group, process);
        if (process) CloseHandle(process);
        if (!assigned) { CloseHandle(group); group = nullptr; }
    }
#endif
    job.write(request);
    job.closeWriteChannel();
    while (!canceled) {
        if (!job.bytesAvailable()) {
            if (job.state() == QProcess::NotRunning) break;
            if (!job.waitForReadyRead(40) && !job.bytesAvailable())
                job.waitForFinished(1);
            continue;
        }
        if (!available.tryAcquire(1, 25)) continue;
        const auto bytes = job.read(32 * 1024);
        // This callback precedes the finished notification on the same receiver.
        // Cancellation can retire the pump with one last delivery still posted.
        QMetaObject::invokeMethod(receiver, [this, bytes] {
            if (!canceled) deliver(this, bytes);
            available.release();
        }, Qt::QueuedConnection);
    }
    if (canceled && job.state() != QProcess::NotRunning) {
#ifdef Q_OS_UNIX
        const auto pid = pid_t(job.processId());
        if (pid > 0 && ::getpgid(pid) == pid) ::kill(-pid, SIGKILL);
#endif
        job.kill();
        job.waitForFinished();
    } else if (job.exitStatus() != QProcess::NormalExit || job.exitCode() != 0) {
        ending = Outcome::WorkerFailed;
    }
#ifdef Q_OS_WIN
    if (group) CloseHandle(group);
#endif
}
}
