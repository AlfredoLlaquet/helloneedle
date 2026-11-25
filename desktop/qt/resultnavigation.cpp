// SPDX-License-Identifier: GPL-3.0-or-later
#include "resultnavigation.h"
#include "resultbrowser.h"
#include "buttonicons.h"
#include "language.h"
#include <QApplication>
#include <QHBoxLayout>
#include <QKeyEvent>
#include <QShortcut>
#include <QToolButton>

namespace hn {
namespace {
enum class Unit { Document, Match };
struct Command {
    const char *id;
    const char *name;
    ButtonIcon icon;
    Unit unit;
    int direction;
    QList<QKeySequence> keys;
};

// Qt calls the physical Control key Meta on macOS and Control elsewhere.
#ifdef Q_OS_MACOS
constexpr auto control = Qt::MetaModifier;
#else
constexpr auto control = Qt::ControlModifier;
#endif

const Command commands[] = {
    {"nextDocumentButton", "Next document", ButtonIcon::NextDocument, Unit::Document, 1,
     {control | Qt::Key_Down}},
    {"nextMatchButton", "Next match", ButtonIcon::NextMatch, Unit::Match, 1,
     {Qt::Key_F3, Qt::AltModifier | Qt::Key_Down}},
    {"previousMatchButton", "Previous match", ButtonIcon::PreviousMatch, Unit::Match, -1,
     {Qt::ShiftModifier | Qt::Key_F3, Qt::AltModifier | Qt::Key_Up}},
    {"previousDocumentButton", "Previous document", ButtonIcon::PreviousDocument, Unit::Document, -1,
     {control | Qt::Key_Up}}
};
}

ResultNavigation::ResultNavigation(ResultBrowser *results, QWidget *parent) : QWidget(parent), browser(results)
{
    setObjectName("resultNavigation");
    auto *layout = new QHBoxLayout(this);
    layout->setContentsMargins(0, 0, 0, 0);
    for (const auto &command : commands) {
        auto *button = new QToolButton(this);
        button->setObjectName(command.id);
        button->setToolButtonStyle(Qt::ToolButtonIconOnly);
        button->setFocusPolicy(Qt::StrongFocus);
        setButtonIcon(button, command.icon);
        auto *shortcut = new QShortcut(button);
        shortcut->setKeys(command.keys);
        shortcut->setContext(Qt::WindowShortcut);
        connect(shortcut, &QShortcut::activated, button, &QToolButton::click);
        connect(button, &QToolButton::clicked, this, [this, &command] {
            browser->navigate(command.direction, command.unit == Unit::Document);
        });
        layout->addWidget(button);
        buttons.append(button);
    }
    layout->addStretch();
    setEnabled(false);
    qApp->installEventFilter(this);
}

void ResultNavigation::retranslate()
{
    for (qsizetype i = 0; i < buttons.size(); ++i) {
        const auto &command = commands[i];
        const auto label = ui(command.name);
        auto keys = command.keys.first().toString(QKeySequence::NativeText);
        if (command.keys.size() == 2)
            keys = ui("%1 or %2").arg(keys, command.keys.last().toString(QKeySequence::NativeText));
        buttons[i]->setAccessibleName(label);
        buttons[i]->setToolTip(QStringLiteral("%1 (%2)").arg(label, keys));
    }
}

bool ResultNavigation::eventFilter(QObject *watched, QEvent *event)
{
    if (event->type() == QEvent::ShortcutOverride && isEnabled()) {
        const auto *focus = qobject_cast<QWidget *>(watched);
        if (focus && focus->window() == window()) {
            // Editors and tree views may claim Ctrl+Arrow as ordinary cursor movement.
            // Reserve the advertised shortcuts within this window, including its preview.
            const auto key = QKeySequence(static_cast<QKeyEvent *>(event)->keyCombination());
            for (const auto &command : commands) {
                if (command.keys.contains(key)) {
                    event->ignore();
                    return true;
                }
            }
        }
    }
    return QWidget::eventFilter(watched, event);
}

}
