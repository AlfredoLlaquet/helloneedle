// SPDX-License-Identifier: GPL-3.0-or-later
#pragma once
#include <QWidget>

class QGridLayout;
class QLabel;
class QPushButton;
class QStatusBar;

namespace hn {
class SearchStatus : public QWidget {
public:
    explicit SearchStatus(QStatusBar *bar);
    QPushButton *detailsButton() const { return details; }
    void retranslate();
    void setMessage(const QString &text, int errors);
protected:
    void resizeEvent(QResizeEvent *event) override;
    void showEvent(QShowEvent *event) override;
private:
    void arrange();
    QStatusBar *bar;
    QGridLayout *grid;
    QLabel *status;
    QLabel *errorCount;
    QPushButton *link;
    QWidget *errorGroup;
    QPushButton *details;
    bool compact = false;
};
}
