// SPDX-License-Identifier: GPL-3.0-or-later
#pragma once
#include "workerclient.h"
#include <QMainWindow>

class QAction;
class QBoxLayout;
class QCheckBox;
class QCloseEvent;
class QComboBox;
class QLabel;
class QLineEdit;
class QMenu;
class QPushButton;
class QScrollArea;
class QSplitter;
class QToolButton;

namespace hn {
class PreviewPane;
class ResultNavigation;
class ResultBrowser;
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
    void refreshStatus();
    void adaptLayout();
    void adjustTextContrast();

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
    QCheckBox *excludeSubdirectories = nullptr;
    QPushButton *browse = nullptr;
    QPushButton *search = nullptr;
    QPushButton *cancel = nullptr;
    QToolButton *options = nullptr;
    QWidget *optionsPane = nullptr;
    QComboBox *mode = nullptr;
    QSplitter *splitter = nullptr;
    QBoxLayout *resultControls = nullptr;
    QComboBox *resultView = nullptr;
    ResultBrowser *results = nullptr;
    ResultNavigation *navigation = nullptr;
    PreviewPane *preview = nullptr;
    QScrollArea *stackedPreview = nullptr;
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
    State state = State::Ready;
    qint64 count = 0;
    qint64 filesScanned = 0;
    qint64 errors = 0;
    QString failure;
};
}
