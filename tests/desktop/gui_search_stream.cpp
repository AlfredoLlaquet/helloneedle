// SPDX-License-Identifier: GPL-3.0-or-later
#include "gui_search_stream.h"
#include <QAbstractButton>
#include <QCoreApplication>
#include <QDir>
#include <QElapsedTimer>
#include <QFile>
#include <QJsonDocument>
#include <QThread>
#include <cstdio>

namespace {
bool writeFile(const QString &path, const QByteArray &bytes)
{
    QFile file(path);
    return file.open(QIODevice::WriteOnly) && file.write(bytes) == bytes.size() && file.flush();
}
}

namespace guiqa {
bool SearchStream::prepare(const QByteArray &first, const QByteArray &rest) const
{
    return folder.isValid() && writeFile(folder.filePath("first.jsonl"), first)
        && writeFile(folder.filePath("rest.jsonl"), rest);
}

void SearchStream::start(QAbstractButton &button) const
{
    const auto original = qgetenv("HELLONEEDLE_WORKER");
    qputenv("HELLONEEDLE_WORKER", QCoreApplication::applicationFilePath().toUtf8());
    button.click();
    qputenv("HELLONEEDLE_WORKER", original);
}

bool SearchStream::resume() const
{
    return writeFile(folder.filePath("resume"), {});
}

QJsonObject SearchStream::request() const
{
    QFile file(folder.filePath("request.json"));
    if (!file.open(QIODevice::ReadOnly)) return {};
    return QJsonDocument::fromJson(file.readAll()).object();
}

int replaySearchStream()
{
    QFile input, output;
    if (!input.open(stdin, QIODevice::ReadOnly) || !output.open(stdout, QIODevice::WriteOnly)) return 1;
    const QByteArray request = input.readAll();
    const QDir directory(QJsonDocument::fromJson(request).object().value("folder").toString());
    QFile first(directory.filePath("first.jsonl")), rest(directory.filePath("rest.jsonl"));
    if (!first.open(QIODevice::ReadOnly) || !rest.open(QIODevice::ReadOnly)
        || !writeFile(directory.filePath("request.json"), request)) return 1;
    const auto send = [&](QFile &file) {
        const auto bytes = file.readAll();
        return output.write(bytes) == bytes.size() && output.flush();
    };
    if (!send(first)) return 1;
    QElapsedTimer timer;
    timer.start();
    while (!directory.exists("resume") && timer.elapsed() < 20000) QThread::msleep(10);
    return directory.exists("resume") && send(rest) ? 0 : 1;
}
}
