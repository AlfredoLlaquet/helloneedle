// SPDX-License-Identifier: GPL-3.0-or-later
#include "application.h"
#include "mainwindow.h"

#include <QApplication>
#include <QCommandLineParser>
#include <QFile>
#include <QLineEdit>
#include <QSettings>
#include <QTemporaryDir>
#include <QTextBrowser>
#include <QTimer>
#include <QTreeWidget>
#include <cstdio>

int main(int argc, char **argv)
{
    QApplication app(argc, argv);
    const bool testing = app.arguments().contains(QStringLiteral("--smoke-test"));
    QTemporaryDir scratch;
    if (testing) {
        if (!scratch.isValid()) return 2;
        QSettings::setDefaultFormat(QSettings::IniFormat);
        QSettings::setPath(QSettings::IniFormat, QSettings::UserScope, scratch.path());
    }
    hn::initializeApplication(app);
    QCommandLineParser parser;
    parser.setApplicationDescription(QStringLiteral("Hello Needle"));
    parser.addHelpOption();
    parser.addVersionOption();
    parser.addOption({"smoke-test", QStringLiteral("Verify the installed interface, worker and text preview.")});
    parser.addPositionalArgument(QStringLiteral("folder"), QStringLiteral("Folder to search."), QStringLiteral("[folder]"));
    parser.process(app);
    hn::MainWindow window;
    if (!parser.positionalArguments().isEmpty())
        window.setFolder(parser.positionalArguments().constFirst());
    window.show();
    if (testing) {
        QFile sample(scratch.filePath(QStringLiteral("sample.txt")));
        if (!sample.open(QIODevice::WriteOnly) || sample.write("first line\nneedle-package-check\n") < 0) return 2;
        sample.close();
        window.setFolder(scratch.path());
        window.findChild<QLineEdit *>("queryEdit")->setText(QStringLiteral("needle-package-check"));
        QTimer::singleShot(0, &window, &hn::MainWindow::startSearch);
        auto *poll = new QTimer(&window);
        QObject::connect(poll, &QTimer::timeout, &window, [&app, &window] {
            const auto *view = window.findChild<QTreeWidget *>("resultsView");
            const auto *preview = window.findChild<QTextBrowser *>("previewText");
            if (window.isVisible() && view->topLevelItemCount() == 1 && !preview->extraSelections().isEmpty()
                && preview->extraSelections().first().cursor.selectedText() == QStringLiteral("needle-package-check")) {
                std::puts("Hello Needle installed GUI, worker and selected text preview passed.");
                app.exit(0);
            }
        });
        poll->start(40);
        QTimer::singleShot(30000, &app, [&app] {
            std::fputs("Hello Needle installed GUI verification timed out.\n", stderr);
            app.exit(3);
        });
    }
    return app.exec();
}
