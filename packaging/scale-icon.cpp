// SPDX-License-Identifier: GPL-3.0-or-later
#include <QCoreApplication>
#include <QDebug>
#include <QDir>
#include <QImage>

int main(int argc, char **argv)
{
    QCoreApplication app(argc, argv);
    const QStringList args = app.arguments();
    if (args.size() < 4)
        return 1;
    const QImage source(args[1], "PNG");
    if (source.isNull() || source.width() != source.height() || !source.hasAlphaChannel()) {
        qCritical() << "Expected a square application icon with transparency:" << args[1];
        return 1;
    }
    const QDir destination(args[2]);
    for (const QString &argument : args.sliced(3)) {
        const int size = argument.toInt();
        const QString file = destination.filePath(argument + ".png");
        if (size < 1 || size > source.width() ||
            !source.scaled(size, size, Qt::KeepAspectRatio, Qt::SmoothTransformation).save(file, "PNG")) {
            qCritical() << "Could not create the application icon at" << file;
            return 1;
        }
    }
    return 0;
}
