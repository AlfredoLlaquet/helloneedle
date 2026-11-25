// SPDX-License-Identifier: GPL-3.0-or-later
#pragma once
#include <QList>
#include <QWidget>

class QToolButton;

namespace hn {
class ResultBrowser;
class ResultNavigation final : public QWidget {
public:
    ResultNavigation(ResultBrowser *results, QWidget *parent);
    void retranslate();

protected:
    bool eventFilter(QObject *watched, QEvent *event) override;

private:
    ResultBrowser *browser;
    QList<QToolButton *> buttons;
};
}
