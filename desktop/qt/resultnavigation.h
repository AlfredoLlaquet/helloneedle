// SPDX-License-Identifier: GPL-3.0-or-later
#pragma once
#include <QList>
#include <QWidget>

class QToolButton;
class QTreeWidget;
class QTreeWidgetItem;

namespace hn {
class ResultNavigation final : public QWidget {
public:
    ResultNavigation(QTreeWidget *results, QWidget *parent);
    void retranslate();
    void selectMatch(QTreeWidgetItem *item);

protected:
    bool eventFilter(QObject *watched, QEvent *event) override;

private:
    void navigate(int direction, bool byDocument);
    QTreeWidget *tree;
    QList<QToolButton *> buttons;
};
}
