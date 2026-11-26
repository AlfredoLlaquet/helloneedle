// SPDX-License-Identifier: GPL-3.0-or-later
#pragma once
#include <QJsonObject>
#include <QWidget>
#include "workerclient.h"

class QLabel;
class QPlainTextEdit;
class QPushButton;
class QStackedWidget;

namespace hn {
class PdfPane;
class PreviewText;
class PreviewTitle;
class PreviewPane : public QWidget {
public:
    explicit PreviewPane(QWidget *parent = nullptr);
    void clear();
    void showMatch(const QJsonObject &event);
    void retranslate();
    void openOriginal();
private:
    void updateTitle();
    void load(const QJsonObject &location, bool first, qint64 start, qint64 length);
    void requestMatchPart(bool first = false);
    void movePart(int direction);
    void updateParts();
    void transferMatch(bool save);
    void present(const QJsonObject &data, bool first);
    void displayError(const QString &message);
    void chooseView(bool page);
    QJsonObject match;
    WorkerClient worker;
    WorkerClient transfer;
    PreviewTitle *name;
    QLabel *directoryLabel;
    QPlainTextEdit *directory;
    QPushButton *open;
    QPushButton *toggle;
    QPushButton *copy;
    QPushButton *save;
    QPushButton *previousPart;
    QPushButton *nextPart;
    QLabel *partLabel;
    QLabel *transferStatus;
    QStackedWidget *stack;
    PreviewText *text;
    PdfPane *pdf;
    bool hasPdf = false;
    bool received = false;
    int textPage = 0;
    qint64 matchPart = 0;
    QJsonObject shownLocation;
    qint64 shownEnd = 0;
    bool showingMatch = true;
};
}
