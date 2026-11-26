// SPDX-License-Identifier: GPL-3.0-or-later
#include "mainwindow.h"
#include "buttonicons.h"
#include "credits.h"
#include "language.h"
#include "performancesettings.h"
#include "previewpane.h"
#include "resultnavigation.h"
#include "resultbrowser.h"
#include "searchstatus.h"
#include <QActionGroup>
#include <QApplication>
#include <QCheckBox>
#include <QCloseEvent>
#include <QComboBox>
#include <QDialog>
#include <QDir>
#include <QFileDialog>
#include <QFormLayout>
#include <QGridLayout>
#include <QLabel>
#include <QLineEdit>
#include <QLocale>
#include <QMenuBar>
#include <QPushButton>
#include <QScreen>
#include <QScrollArea>
#include <QSettings>
#include <QSplitter>
#include <QStandardPaths>
#include <QStatusBar>
#include <QTextBrowser>
#include <QToolButton>
#include <QVBoxLayout>
#include <cmath>
#include <limits>

namespace {
class StackedPreview final : public QScrollArea {
public:
    using QScrollArea::QScrollArea;

    QSize minimumSizeHint() const override
    {
        return widget() ? static_cast<hn::PreviewPane *>(widget())->minimumViewportSizeHint()
                        : QScrollArea::minimumSizeHint();
    }
};
}

namespace hn {
MainWindow::MainWindow(QWidget *parent) : QMainWindow(parent), worker(this)
{
    setObjectName("mainWindow");
    setWindowTitle(QStringLiteral("Hello Needle"));
    buildControls();
    buildMenus();
    readSettings();
    retranslate();
    adjustTextContrast();
    queryEdit->setFocus();
}

void MainWindow::buildControls()
{
    auto *body = new QWidget(this);
    auto *layout = new QVBoxLayout(body);
    layout->setContentsMargins(12, 10, 12, 6);
    layout->setSpacing(7);
    auto *fields = new QGridLayout;
    fields->setColumnStretch(1, 1);
    auto line = [body](const char *id) {
        auto *widget = new QLineEdit(body);
        widget->setObjectName(id);
        widget->setMinimumWidth(35);
        widget->setMaxLength(std::numeric_limits<int>::max());
        return widget;
    };
    folderEdit = line("folderEdit");
    folderLabel = new QLabel(body);
    folderLabel->setBuddy(folderEdit);
    browse = new QPushButton(body);
    browse->setObjectName("browseButton");
    setButtonIcon(browse, ButtonIcon::Browse);
    fields->addWidget(folderLabel, 0, 0);
    fields->addWidget(folderEdit, 0, 1, 1, 2);
    fields->addWidget(browse, 0, 3);
    queryEdit = line("queryEdit");
    queryEdit->setClearButtonEnabled(true);
    queryLabel = new QLabel(body);
    queryLabel->setBuddy(queryEdit);
    search = new QPushButton(body);
    search->setObjectName("searchButton");
    setButtonIcon(search, ButtonIcon::Search);
    search->setDefault(true);
    cancel = new QPushButton(body);
    cancel->setObjectName("cancelButton");
    setButtonIcon(cancel, ButtonIcon::Cancel);
    fields->addWidget(queryLabel, 1, 0);
    fields->addWidget(queryEdit, 1, 1);
    fields->addWidget(search, 1, 2);
    fields->addWidget(cancel, 1, 3);
    layout->addLayout(fields);
    auto *modes = new QHBoxLayout;
    options = new QToolButton(body);
    options->setObjectName("optionsButton");
    options->setToolButtonStyle(Qt::ToolButtonTextBesideIcon);
    setButtonIcon(options, ButtonIcon::Options);
    options->setCheckable(true);
    modes->addWidget(options);
    modeLabel = new QLabel(body);
    modeLabel->setObjectName("modeLabel");
    mode = new QComboBox(body);
    mode->setObjectName("modeCombo");
    modeLabel->setBuddy(mode);
    mode->addItem("", "document");
    mode->addItem("", "source");
    modes->addWidget(modeLabel);
    modes->addWidget(mode);
    modes->addStretch();
    layout->addLayout(modes);
    optionsPane = new QWidget(body);
    optionsPane->setObjectName("optionsPane");
    auto *grid = new QGridLayout(optionsPane);
    grid->setContentsMargins(0, 0, 0, 2);
    auto checkbox = [this, grid](const char *id, int row, int col) {
        auto *box = new QCheckBox(optionsPane);
        box->setObjectName(id);
        grid->addWidget(box, row, col);
        return box;
    };
    regex = checkbox("regexCheck", 0, 0);
    respectCase = checkbox("caseCheck", 0, 1);
    hidden = checkbox("hiddenCheck", 1, 0);
    ignored = checkbox("ignoredCheck", 1, 1);
    filenames = checkbox("filenamesCheck", 2, 0);
    excludeSubdirectories = checkbox("excludeSubdirectoriesCheck", 2, 1);
    patternEdit = line("pathPatternEdit");
    patternLabel = new QLabel(optionsPane);
    patternLabel->setBuddy(patternEdit);
    grid->addWidget(patternLabel, 3, 0);
    grid->addWidget(patternEdit, 3, 1);
    grid->setColumnStretch(1, 1);
    layout->addWidget(optionsPane);
    optionsPane->setVisible(false);
    splitter = new QSplitter(Qt::Horizontal, body);
    splitter->setObjectName("resultSplitter");
    splitter->setChildrenCollapsible(false);
    splitter->setHandleWidth(7);
    auto *left = new QWidget(splitter);
    auto *leftLayout = new QVBoxLayout(left);
    leftLayout->setContentsMargins(0, 0, 0, 0);
    resultsLabel = new QLabel(left);
    QFont heading = resultsLabel->font();
    heading.setBold(true);
    resultsLabel->setFont(heading);
    auto *resultHeader = new QFormLayout;
    resultHeader->setRowWrapPolicy(QFormLayout::WrapLongRows);
    resultHeader->setFieldGrowthPolicy(QFormLayout::FieldsStayAtSizeHint);
    resultHeader->setLabelAlignment(Qt::AlignLeft | Qt::AlignVCenter);
    resultHeader->setFormAlignment(Qt::AlignLeft | Qt::AlignTop);
    resultView = new QComboBox(left);
    resultView->setObjectName("resultViewCombo");
    resultView->setSizeAdjustPolicy(QComboBox::AdjustToContents);
    resultView->setSizePolicy(QSizePolicy::Minimum, resultView->sizePolicy().verticalPolicy());
    resultView->addItem("", "names");
    resultView->addItem("", "paths");
    resultView->addItem("", "tree");
    resultHeader->addRow(resultsLabel, resultView);
    resultControls = new QBoxLayout(QBoxLayout::TopToBottom);
    resultControls->addLayout(resultHeader);
    leftLayout->addLayout(resultControls);
    results = new ResultBrowser(left);
    navigation = new ResultNavigation(results, left);
    resultControls->addWidget(navigation);
    leftLayout->addWidget(results);
    stackedPreview = new StackedPreview(body);
    stackedPreview->setObjectName("previewScrollArea");
    stackedPreview->setFrameShape(QFrame::NoFrame);
    stackedPreview->setWidgetResizable(true);
    stackedPreview->setFocusPolicy(Qt::NoFocus);
    stackedPreview->hide();
    preview = new PreviewPane(splitter);
    splitter->addWidget(left);
    splitter->addWidget(preview);
    splitter->setStretchFactor(0, 2);
    splitter->setStretchFactor(1, 3);
    layout->addWidget(splitter, 1);
    setCentralWidget(body);
    status = new SearchStatus(statusBar());
    statusBar()->addPermanentWidget(status, 1);
    connect(status->detailsButton(), &QPushButton::clicked, results, &ResultBrowser::showErrors);
    connect(browse, &QPushButton::clicked, this, &MainWindow::chooseFolder);
    connect(search, &QPushButton::clicked, this, &MainWindow::startSearch);
    connect(cancel, &QPushButton::clicked, this, &MainWindow::cancelSearch);
    connect(queryEdit, &QLineEdit::returnPressed, this, &MainWindow::startSearch);
    connect(folderEdit, &QLineEdit::returnPressed, this, [this]{ queryEdit->setFocus(); });
    connect(patternEdit, &QLineEdit::returnPressed, this, &MainWindow::startSearch);
    connect(options, &QToolButton::toggled, optionsPane, &QWidget::setVisible);
    results->selectionChanged = [this](const QJsonObject &match) {
        openAction->setEnabled(!match.isEmpty());
        if (match.isEmpty()) preview->clear();
        else preview->showMatch(match);
    };
    results->storageFailure = [this](const QString &message) {
        count = results->count();
        errors = results->errors();
        failure = message;
        state = State::Error;
        worker.stop();
        refreshStatus();
    };
    connect(resultView, &QComboBox::currentIndexChanged, this, [this](int index) {
        results->setOrder(index == 2 ? ResultOrder::Tree : index == 1 ? ResultOrder::Paths : ResultOrder::Names);
        QSettings().setValue("results/view", resultView->currentData());
    });
}

void MainWindow::buildMenus()
{
    fileMenu = menuBar()->addMenu(QString());
    fileMenu->setObjectName("fileMenu");
    searchMenu = menuBar()->addMenu(QString());
    searchMenu->setObjectName("searchMenu");
    languageMenu = menuBar()->addMenu(QString());
    languageMenu->setObjectName("languageMenu");
    helpMenu = menuBar()->addMenu(QString());
    helpMenu->setObjectName("helpMenu");
    auto action = [this](QMenu *menu, const char *id, const auto &slot) {
        auto *a = menu->addAction(QString());
        a->setObjectName(id);
        connect(a, &QAction::triggered, this, slot);
        return a;
    };
    folderAction = action(fileMenu, "folderAction", [this]{ chooseFolder(); });
    folderAction->setShortcut(QKeySequence::Open);
    openAction = action(fileMenu, "openAction", [this]{ preview->openOriginal(); });
    openAction->setEnabled(false);
    settingsAction = action(fileMenu, "settingsAction", [this]{ showPerformanceSettings(this); });
    settingsAction->setMenuRole(QAction::NoRole);
    fileMenu->addSeparator();
    quitAction = action(fileMenu, "quitAction", [this]{ close(); });
    quitAction->setShortcut(QKeySequence::Quit);
    quitAction->setMenuRole(QAction::QuitRole);
    focusAction = action(searchMenu, "focusSearchAction", [this]{ queryEdit->setFocus(); queryEdit->selectAll(); });
    focusAction->setShortcut(QKeySequence::Find);
    searchAction = action(searchMenu, "searchAction", [this]{ startSearch(); });
    cancelAction = action(searchMenu, "cancelAction", [this]{ cancelSearch(); });
    cancelAction->setShortcut(QKeySequence(Qt::Key_Escape));
    englishAction = action(languageMenu, "englishAction", [this]{ selectLanguage("en"); });
    spanishAction = action(languageMenu, "spanishAction", [this]{ selectLanguage("es"); });
    auto *languages = new QActionGroup(this);
    for (auto *entry : {englishAction, spanishAction}) {
        entry->setCheckable(true);
        languages->addAction(entry);
    }
    englishAction->setText(QStringLiteral("English"));
    spanishAction->setText(QStringLiteral("Español"));
    aboutAction = action(helpMenu, "aboutAction", [this]{ showCredits(this, false); });
    aboutAction->setMenuRole(QAction::AboutRole);
    licensesAction = action(helpMenu, "licensesAction", [this]{ showCredits(this, true); });
}

void MainWindow::readSettings()
{
    QSettings settings;
    QString initial = QStandardPaths::writableLocation(QStandardPaths::DocumentsLocation);
    if (!QDir(initial).exists() || initial.isEmpty()) initial = QDir::homePath();
    setFolder(settings.value("search/folder", initial).toString());
    regex->setChecked(settings.value("search/regex", false).toBool());
    respectCase->setChecked(settings.value("search/caseSensitive", false).toBool());
    hidden->setChecked(settings.value("search/hidden", false).toBool());
    ignored->setChecked(settings.value("search/ignored", false).toBool());
    filenames->setChecked(settings.value("search/filenames", false).toBool());
    excludeSubdirectories->setChecked(settings.value("search/excludeSubdirectories", false).toBool());
    resultView->setCurrentIndex(qMax(0, resultView->findData(settings.value("results/view", "names"))));
    patternEdit->setText(settings.value("search/pathPattern").toString());
    options->setChecked(settings.value("window/options", false).toBool());
    mode->setCurrentIndex(mode->findData(settings.value("search/mode", "document")));
    auto *display = screen();
    const QSize available = display ? display->availableGeometry().size() : QSize(1024, 768);
    resize(qMin(1040, qRound(available.width() * .91)), qMin(710, qRound(available.height() * .85)));
    if (settings.contains("window/geometry")) restoreGeometry(settings.value("window/geometry").toByteArray());
    const auto savedSplitter = settings.value("window/splitter").toByteArray();
    if (!savedSplitter.isEmpty()) splitter->restoreState(savedSplitter);
    else splitter->setSizes({380, 610});
    adaptLayout();
}

void MainWindow::writeSettings()
{
    QSettings settings;
    settings.setValue("search/folder", folderEdit->text());
    settings.setValue("search/mode", mode->currentData());
    settings.setValue("search/regex", regex->isChecked());
    settings.setValue("search/caseSensitive", respectCase->isChecked());
    settings.setValue("search/hidden", hidden->isChecked());
    settings.setValue("search/ignored", ignored->isChecked());
    settings.setValue("search/filenames", filenames->isChecked());
    settings.setValue("search/excludeSubdirectories", excludeSubdirectories->isChecked());
    settings.setValue("search/pathPattern", patternEdit->text());
    settings.setValue("window/options", options->isChecked());
    settings.setValue("window/geometry", saveGeometry());
    settings.setValue("window/splitter", splitter->saveState());
}

void MainWindow::setFolder(const QString &path) { folderEdit->setText(QDir::toNativeSeparators(path)); }

void MainWindow::chooseFolder()
{
    QFileDialog dialog(this);
    const auto os = QLocale(systemLanguage(QLocale::system().uiLanguages())).language();
    const auto selectedLanguage = currentLanguage() == "es" ? QLocale::Spanish : QLocale::English;
    if (os != selectedLanguage) dialog.setOption(QFileDialog::DontUseNativeDialog);
    dialog.setWindowTitle(ui("Choose a folder"));
    dialog.setFileMode(QFileDialog::Directory);
    dialog.setOption(QFileDialog::ShowDirsOnly);
    dialog.setDirectory(folderEdit->text());
    dialog.setLabelText(QFileDialog::LookIn, ui("Look in:"));
    dialog.setLabelText(QFileDialog::FileName, ui("Folder:"));
    dialog.setLabelText(QFileDialog::Accept, ui("Select folder"));
    dialog.setLabelText(QFileDialog::Reject, ui("Cancel"));
    if (dialog.exec() != QDialog::Accepted || dialog.selectedFiles().isEmpty()) return;
    setFolder(dialog.selectedFiles().constFirst());
    queryEdit->setFocus();
}

void MainWindow::selectLanguage(const QString &code)
{
    QSettings().setValue("interface/language", code);
    setLanguage(code);
    retranslate();
    const auto match = results->selectedMatch();
    if (!match.isEmpty()) preview->showMatch(match);
}

void MainWindow::retranslate()
{
    folderLabel->setText(ui("Folder"));
    queryLabel->setText(ui("Find"));
    browse->setText(ui("Browse…"));
    search->setText(ui("Search"));
    cancel->setText(ui("Cancel"));
    options->setText(ui("Options"));
    options->setAccessibleName(ui("Search options"));
    modeLabel->setText(ui("Search in"));
    mode->setAccessibleName(ui("Search in"));
    mode->setItemText(0, ui("Document"));
    mode->setItemText(1, ui("Source"));
    mode->setToolTip(ui("Search readable text or the saved HTML and Markdown markup."));
    folderEdit->setPlaceholderText(ui("Choose a local folder"));
    folderEdit->setAccessibleName(ui("Folder"));
    queryEdit->setPlaceholderText(ui("Word or phrase"));
    queryEdit->setAccessibleName(ui("Search query"));
    regex->setText(ui("Regular expression"));
    respectCase->setText(ui("Case sensitive"));
    hidden->setText(ui("Include hidden files"));
    ignored->setText(ui("Include ignored files"));
    filenames->setText(ui("Search file names"));
    excludeSubdirectories->setText(ui("Exclude subdirectories"));
    patternLabel->setText(ui("Path filter"));
    patternEdit->setAccessibleName(ui("Path filter"));
    patternEdit->setPlaceholderText(ui("For example, *.md"));
    resultsLabel->setText(ui("Results"));
    resultView->setAccessibleName(ui("Result view"));
    resultView->setToolTip(ui("Result view"));
    resultView->setItemText(0, ui("File names"));
    resultView->setItemText(1, ui("File paths"));
    resultView->setItemText(2, ui("Folder tree"));
    navigation->retranslate();
    status->retranslate();
    fileMenu->setTitle(ui("&File"));
    searchMenu->setTitle(ui("&Search"));
    languageMenu->setTitle(ui("&Language"));
    helpMenu->setTitle(ui("&Help"));
    folderAction->setText(ui("Choose folder…"));
    openAction->setText(ui("Open original"));
    settingsAction->setText(ui("Settings…"));
    quitAction->setText(ui("Quit"));
    focusAction->setText(ui("Focus search field"));
    searchAction->setText(ui("Search"));
    cancelAction->setText(ui("Cancel search"));
    aboutAction->setText(ui("About Hello Needle"));
    licensesAction->setText(ui("Licenses"));
    englishAction->setChecked(currentLanguage() == "en");
    spanishAction->setChecked(currentLanguage() == "es");
    results->retranslate();
    preview->retranslate();
    refreshStatus();
}

void MainWindow::startSearch()
{
    worker.stop();
    preview->clear();
    openAction->setEnabled(false);
    failure.clear();
    count = 0;
    filesScanned = 0;
    errors = 0;
    results->reset();
    if (!failure.isEmpty()) return;
    if (queryEdit->text().isEmpty() || folderEdit->text().isEmpty()) {
        state = State::Error;
        failure = queryEdit->text().isEmpty() ? ui("Enter a word or phrase.") : ui("Choose a folder.");
        (queryEdit->text().isEmpty() ? queryEdit : folderEdit)->setFocus();
        refreshStatus();
        return;
    }
    state = State::Searching;
    refreshStatus();
    QJsonObject request{{"folder", QDir::fromNativeSeparators(folderEdit->text())}, {"query", queryEdit->text()},
        {"regex", regex->isChecked()}, {"caseSensitive", respectCase->isChecked()},
        {"hidden", hidden->isChecked()}, {"ignored", ignored->isChecked()}, {"filenames", filenames->isChecked()},
        {"excludeSubdirectories", excludeSubdirectories->isChecked()},
        {"pathPattern", patternEdit->text()}, {"mode", mode->currentData().toString()}};
    applyPerformanceSettings(request);
    worker.start("search", request, [this](const QJsonObject &data) {
        const auto type = data.value("type").toString();
        if (type == "progress" || type == "done")
            filesScanned = data.value("files").toInteger(data.value("documentsScanned").toInteger(filesScanned));
        if (type == "match") { results->append(data); count = results->count(); }
        else if (type == "discard") { results->discard(data); count = results->count(); }
        else if (type == "error") {
            results->appendError(data);
            errors = results->errors();
            const auto code = data.value("code").toString();
            if (code == "query" || code == "folder" || code == "request") failure = workerError(data);
        } else if (type == "done") {
            count = data.value("count").toInteger(count);
            results->settle();
            state = failure.isEmpty() ? State::Completed : State::Error;
        }
        if (type != "match" || count % 64 == 1) refreshStatus();
    }, [this](const QString &error) {
        if (!error.isEmpty() && failure.isEmpty()) failure = error;
        if (state == State::Searching) {
            if (failure.isEmpty()) failure = ui("The worker stopped before the search finished.");
            state = State::Error;
        }
        if (!failure.isEmpty()) state = State::Error;
        refreshStatus();
    });
}

void MainWindow::cancelSearch()
{
    if (state != State::Searching) return;
    worker.stop();
    results->settle();
    if (state == State::Error) return;
    state = State::Canceled;
    refreshStatus();
}

void MainWindow::refreshStatus()
{
    const bool active = state == State::Searching;
    cancel->setEnabled(active);
    cancelAction->setEnabled(active);
    search->setEnabled(!active);
    navigation->setEnabled(count > 0);
    const QString scanned = filesScanned == 1 ? ui("1 file scanned")
                                                 : ui("%1 files scanned").arg(filesScanned);
    const QString matches = count == 1 ? ui("1 match") : ui("%1 matches").arg(count);
    const QString counts = scanned + QStringLiteral(" · ") + matches;
    QString message;
    switch (state) {
    case State::Ready: message = ui("Ready to search."); break;
    case State::Searching: message = ui("Searching… %1").arg(counts); break;
    case State::Canceled: message = ui("Search canceled.") + QStringLiteral(" ") + counts; break;
    case State::Error: message = failure; break;
    case State::Completed:
        message = counts;
        break;
    }
    status->setMessage(message, errors);
}

void MainWindow::adaptLayout()
{
    if (!preview) return;
    // A scaled 1024×768 desktop can be both narrow and too short for two
    // stacked panes. Use its horizontal space if both panes fit side by side.
    constexpr int previewGap = 24;
    const auto margins = centralWidget()->layout()->contentsMargins();
    const auto *selector = resultControls->itemAt(0);
    const int listMinimum = qMax(results->minimumSizeHint().width(),
                                 qMax(selector->minimumSize().width(), navigation->minimumSizeHint().width()));
    const int horizontalMinimum = margins.left() + margins.right() + splitter->handleWidth()
        + listMinimum + preview->minimumSizeHint().width()
        - preview->layout()->contentsMargins().left() + previewGap;
    // Switch before a stacked layout reaches its minimum height, including
    // when a tall window is resized down on a small native screen.
    const bool compactLandscape = width() >= 560 && width() > horizontalMinimum && width() > height();
    const auto orientation = width() >= 630 || compactLandscape ? Qt::Horizontal : Qt::Vertical;
    const auto controlsWidth = selector->sizeHint().width() + navigation->sizeHint().width()
        + qMax(0, resultControls->spacing());
    const bool oneRow = orientation == Qt::Vertical && width() - margins.left() - margins.right() >= controlsWidth;
    resultControls->setDirection(oneRow ? QBoxLayout::LeftToRight : QBoxLayout::TopToBottom);
    const bool stacked = orientation == Qt::Vertical;
    preview->layout()->setContentsMargins(stacked ? 0 : previewGap, stacked ? previewGap : 0, 0, 0);
    if (stacked != (stackedPreview->widget() != nullptr)) {
        QWidget *focus = QApplication::focusWidget();
        const bool restoreFocus = focus && preview->isAncestorOf(focus);
        if (stacked) {
            splitter->replaceWidget(1, stackedPreview);
            preview->setScrollArea(stackedPreview);
            stackedPreview->setWidget(preview);
            stackedPreview->show();
        } else {
            stackedPreview->takeWidget();
            preview->setScrollArea(nullptr);
            splitter->replaceWidget(1, preview);
            stackedPreview->setParent(centralWidget());
            stackedPreview->hide();
        }
        preview->show();
        if (restoreFocus) focus->setFocus();
    }
    if (splitter->orientation() != orientation) splitter->setOrientation(orientation);
}

void MainWindow::resizeEvent(QResizeEvent *event)
{
    QMainWindow::resizeEvent(event);
    adaptLayout();
}

void MainWindow::closeEvent(QCloseEvent *event)
{
    writeSettings();
    worker.stop();
    preview->clear();
    QMainWindow::closeEvent(event);
}

void MainWindow::adjustTextContrast()
{
    if (!queryEdit || !folderEdit || !patternEdit || !preview) return;
    const auto lum = [](const QColor &c) {
        const auto linear = [](double s) { return s <= .04045 ? s / 12.92 : std::pow((s + .055) / 1.055, 2.4); };
        return .2126 * linear(c.redF()) + .7152 * linear(c.greenF()) + .0722 * linear(c.blueF());
    };
    // Include the QTextBrowser's empty-state prompt: several native palettes
    // supply a translucent gray that is hard to read on the viewport base.
    const QList<QWidget *> fields{folderEdit, queryEdit, patternEdit, preview->findChild<QTextBrowser *>()};
    constexpr double target = 5.0; // Leave room for antialiasing at small native font sizes.
    for (auto *edit : fields) {
        auto palette = QApplication::palette(edit);
        for (auto group : {QPalette::Active, QPalette::Inactive}) {
            QColor base = palette.color(group, QPalette::Base);
            const auto ratio = [&](const QColor &c) {
                auto a = lum(base), b = lum(c);
                return (qMax(a,b) + .05) / (qMin(a,b) + .05);
            };
            QColor ink = palette.color(group, QPalette::Text);
            if (ratio(ink) < target) ink = ratio(Qt::white) > ratio(Qt::black) ? Qt::white : Qt::black;
            palette.setColor(group, QPalette::Text, ink);
            QColor hint = palette.color(group, QPalette::PlaceholderText);
            const double alpha = hint.alphaF();
            hint.setRgbF(alpha*hint.redF() + (1-alpha)*base.redF(), alpha*hint.greenF() + (1-alpha)*base.greenF(),
                         alpha*hint.blueF() + (1-alpha)*base.blueF());
            for (int step = 0; ratio(hint) < target && step < 16; ++step)
                hint.setRgb((hint.red()+ink.red())/2, (hint.green()+ink.green())/2, (hint.blue()+ink.blue())/2);
            palette.setColor(group, QPalette::PlaceholderText, hint);
        }
        edit->setPalette(palette);
    }
}

void MainWindow::changeEvent(QEvent *event)
{
    QMainWindow::changeEvent(event);
    if (event->type() == QEvent::ApplicationPaletteChange || event->type() == QEvent::PaletteChange)
        adjustTextContrast();
}
}
