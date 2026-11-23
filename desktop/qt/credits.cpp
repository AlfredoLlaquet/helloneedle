// SPDX-License-Identifier: GPL-3.0-or-later
#include "credits.h"
#include "language.h"
#include <QApplication>
#include <QDialog>
#include <QDialogButtonBox>
#include <QDirIterator>
#include <QFile>
#include <QHBoxLayout>
#include <QLabel>
#include <QListWidget>
#include <QPlainTextEdit>
#include <QPushButton>
#include <QSplitter>
#include <QVBoxLayout>

static void registerLicenses()
{
    static const bool registered = [] { Q_INIT_RESOURCE(hello_needle_licenses); return true; }();
    Q_UNUSED(registered);
}

namespace hn {
void showCredits(QWidget *parent, bool licenses)
{
    registerLicenses();
    QDialog dialog(parent);
    dialog.setWindowTitle(licenses ? ui("Licenses") : ui("About Hello Needle"));
    dialog.setObjectName(licenses ? "licensesDialog" : "aboutDialog");
    auto *layout = new QVBoxLayout(&dialog);
    if (licenses) {
        auto *split = new QSplitter(&dialog);
        auto *list = new QListWidget(split);
        auto *view = new QPlainTextEdit(split);
        list->setObjectName("licenseList");
        list->setAccessibleName(ui("License files"));
        view->setObjectName("licenseText");
        view->setAccessibleName(ui("License text"));
        view->setReadOnly(true);
        split->setStretchFactor(1, 1);
        layout->addWidget(split);
        const QDir executable(QCoreApplication::applicationDirPath());
        const QStringList roots{QStringLiteral(":/licenses"), executable.filePath("../share/doc/helloneedle"),
                                executable.filePath("../Resources/licenses")};
        QMap<QString, QString> paths;
        for (const auto &root : roots) {
            QDirIterator it(root, QDir::Files, QDirIterator::Subdirectories);
            while (it.hasNext()) {
                const auto path = it.next();
                paths.insert(QDir(root).relativeFilePath(path), path);
            }
        }
        QStringList names = paths.keys();
        for (const auto &id : {"NOTICE.md", "LICENSE"}) {
            if (names.removeOne(QString::fromLatin1(id))) names.prepend(QString::fromLatin1(id));
        }
        for (const auto &name : names) {
            auto *item = new QListWidgetItem(name, list);
            item->setData(Qt::UserRole, paths[name]);
        }
        QObject::connect(list, &QListWidget::currentItemChanged, &dialog, [view](QListWidgetItem *item) {
            if (!item) return;
            QFile file(item->data(Qt::UserRole).toString());
            view->setPlainText(file.open(QIODevice::ReadOnly) ? QString::fromUtf8(file.readAll())
                                                               : ui("The license text could not be opened."));
        });
        list->setCurrentRow(0);
        dialog.resize(760, 480);
    } else {
        auto *heading = new QLabel(QStringLiteral("Hello Needle ") + QCoreApplication::applicationVersion(), &dialog);
        QFont title = heading->font();
        title.setPointSizeF(title.pointSizeF() * 1.45);
        title.setBold(true);
        heading->setFont(title);
        layout->addWidget(heading);
        auto *description = new QLabel(ui("Search inside local documents.") + "\n\n" +
            ui("Created by Alfredo Llaquet-Alsina.") + "\n" + ui("Programmed by ChatGPT.") + "\n\n" +
            ui("Based on Clapgrep by Leopold Luley and contributors.") + "\n" +
            ui("GNU General Public License, version 3 or later."), &dialog);
        description->setTextInteractionFlags(Qt::TextSelectableByMouse);
        layout->addWidget(description);
    }
    auto *buttons = new QDialogButtonBox(&dialog);
    buttons->addButton(ui("Close"), QDialogButtonBox::RejectRole);
    if (!licenses) {
        auto *button = buttons->addButton(ui("Licenses"), QDialogButtonBox::ActionRole);
        QObject::connect(button, &QPushButton::clicked, &dialog, [&dialog]{ showCredits(&dialog, true); });
    }
    QObject::connect(buttons, &QDialogButtonBox::rejected, &dialog, &QDialog::reject);
    layout->addWidget(buttons);
    dialog.exec();
}
}
