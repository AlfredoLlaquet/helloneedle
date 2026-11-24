// SPDX-License-Identifier: GPL-3.0-or-later
#pragma once
#include <QJsonObject>
#include <QWidget>
#include "workerclient.h"

class QLabel;
class QPushButton;
class QStackedWidget;
class QTextBrowser;

namespace hn {
class PdfPane;
class PreviewPane : public QWidget {
public:
    explicit PreviewPane(QWidget *parent = nullptr);
    void clear();
    void showMatch(const QJsonObject &event);
    void retranslate();
    void openOriginal();
private:
    void updateTitle();
    void load(const QJsonObject &location, bool first);
    void present(const QJsonObject &data, bool first);
    void displayError(const QString &message);
    void chooseView(bool page);
    QJsonObject match;
    WorkerClient worker;
    QLabel *name;
    QPushButton *open;
    QPushButton *toggle;
    QStackedWidget *stack;
    QTextBrowser *text;
    PdfPane *pdf;
    bool hasPdf = false;
    bool received = false;
    int textPage = 0;
};
}
