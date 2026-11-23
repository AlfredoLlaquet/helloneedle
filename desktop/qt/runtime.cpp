// SPDX-License-Identifier: GPL-3.0-or-later
#include "runtime.h"

#include <QCoreApplication>
#include <QDir>
#include <QFileInfo>
#include <QProcess>
#include <QProcessEnvironment>
#include <QStandardPaths>

namespace hn {
QString findTool(const QString &name)
{
    QString file = name;
#ifdef Q_OS_WIN
    file += QStringLiteral(".exe");
#endif
    const QDir app(QCoreApplication::applicationDirPath());
    QStringList directories{app.path()};
    for (const auto key : {"HELLONEEDLE_WORKER", "HELLO_NEEDLE_WORKER"}) {
        const QString worker = qEnvironmentVariable(key);
        if (worker.isEmpty()) continue;
        if (name == QStringLiteral("helloneedle-worker")) return worker;
        // The native test driver can run outside the installed application's bundle.
        if (QDir::isAbsolutePath(worker)) directories << QFileInfo(worker).absolutePath();
        break;
    }
    directories << app.filePath("../libexec/helloneedle") << app.filePath("../libexec");
#ifdef Q_OS_MACOS
    directories << QStringLiteral("/opt/local/bin") << QStringLiteral("/opt/homebrew/bin") << QStringLiteral("/usr/local/bin");
#endif
    for (const auto &directory : directories) {
        const QFileInfo candidate(QDir(directory).filePath(file));
        if (candidate.isFile() && candidate.isExecutable()) return candidate.absoluteFilePath();
    }
    return QStandardPaths::findExecutable(file);
}

void configurePdfRenderer(QProcess &process, const QString &program)
{
#if defined(Q_OS_MACOS) || defined(Q_OS_WIN)
    const QDir bin = QFileInfo(program).absoluteDir();
#ifdef Q_OS_MACOS
    const QFileInfo config(bin.filePath("../Resources/fonts/fonts.conf"));
#else
    const QFileInfo config(bin.filePath("../etc/fonts/fonts.conf"));
#endif
    if (config.isFile() && !qEnvironmentVariableIsSet("FONTCONFIG_FILE")) {
        auto env = QProcessEnvironment::systemEnvironment();
        env.insert(QStringLiteral("FONTCONFIG_FILE"), QDir::toNativeSeparators(config.absoluteFilePath()));
        env.insert(QStringLiteral("FONTCONFIG_PATH"), QDir::toNativeSeparators(config.absolutePath()));
        process.setProcessEnvironment(env);
    }
#else
    Q_UNUSED(process);
    Q_UNUSED(program);
#endif
}
}
