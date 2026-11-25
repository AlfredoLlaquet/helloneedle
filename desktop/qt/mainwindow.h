// SPDX-License-Identifier: GPL-3.0-or-later
#pragma once
#include "workerclient.h"
#include <QHash>
#include <QMainWindow>

class QAction;
class QCheckBox;
class QCloseEvent;
class QComboBox;
class QLabel;
class QLineEdit;
class QMenu;
class QPushButton;
class QSplitter;
class QToolButton;
class QTreeWidget;
class QTreeWidgetItem;

namespace hn {
class PreviewPane;
class ResultNavigation;
class SearchStatus;
class MainWindow : public QMainWindow {
public:
    explicit MainWindow(QWidget *parent = nullptr);
    void setFolder(const QString &path);
    void startSearch();
    void cancelSearch();
protected:
    void closeEvent(QCloseEvent *event) override;
    void resizeEvent(QResizeEvent *event) override;
    void changeEvent(QEvent *event) override;
private:
    enum class State { Ready, Searching, Completed, Canceled, Error };
    void buildControls();
    void buildMenus();
    void readSettings();
    void writeSettings();
    void chooseFolder();
    void selectLanguage(const QString &code);
    void retranslate();
    void addMatch(const QJsonObject &event);
    void discardContentMatches(const QJsonObject &file);
    void displayMatch(QTreeWidgetItem *item);
    void showErrors();
    void refreshStatus();
    void adaptLayout();
    void adjustTextContrast();
    QString matchLabel(const QJsonObject &match) const;

    WorkerClient worker;
    QLineEdit *folderEdit = nullptr;
    QLineEdit *queryEdit = nullptr;
    QLineEdit *patternEdit = nullptr;
    QLabel *folderLabel = nullptr;
    QLabel *queryLabel = nullptr;
    QLabel *patternLabel = nullptr;
    QLabel *modeLabel = nullptr;
    QLabel *resultsLabel = nullptr;
    SearchStatus *status = nullptr;
    QCheckBox *regex = nullptr;
    QCheckBox *respectCase = nullptr;
    QCheckBox *hidden = nullptr;
    QCheckBox *ignored = nullptr;
    QCheckBox *filenames = nullptr;
    QPushButton *browse = nullptr;
    QPushButton *search = nullptr;
    QPushButton *cancel = nullptr;
    QToolButton *options = nullptr;
    QWidget *optionsPane = nullptr;
    QComboBox *mode = nullptr;
    QSplitter *splitter = nullptr;
    QTreeWidget *results = nullptr;
    ResultNavigation *navigation = nullptr;
    PreviewPane *preview = nullptr;
    QMenu *fileMenu = nullptr;
    QMenu *searchMenu = nullptr;
    QMenu *languageMenu = nullptr;
    QMenu *helpMenu = nullptr;
    QAction *folderAction = nullptr;
    QAction *openAction = nullptr;
    QAction *settingsAction = nullptr;
    QAction *quitAction = nullptr;
    QAction *searchAction = nullptr;
    QAction *focusAction = nullptr;
    QAction *cancelAction = nullptr;
    QAction *aboutAction = nullptr;
    QAction *licensesAction = nullptr;
    QAction *englishAction = nullptr;
    QAction *spanishAction = nullptr;
    QHash<QString, QTreeWidgetItem *> groups;
    QList<QJsonObject> problems;
    State state = State::Ready;
    qsizetype count = 0;
    qint64 filesScanned = 0;
    int errors = 0;
    bool truncated = false;
    QString failure;
};
}
