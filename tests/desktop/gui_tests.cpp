// SPDX-License-Identifier: GPL-3.0-or-later
#include "application.h"
#include "language.h"
#include "mainwindow.h"
#include "searchstatus.h"
#include "gui_appearance.h"
#include "gui_icon.h"
#include "gui_capture.h"
#include "gui_search_stream.h"
#include "picker_native.h"

#include <QAbstractButton>
#include <QAction>
#include <QApplication>
#include <QCheckBox>
#include <QClipboard>
#include <QComboBox>
#include <QDesktopServices>
#include <QDialogButtonBox>
#include <QDir>
#include <QElapsedTimer>
#include <QEnterEvent>
#include <QFile>
#include <QFileDialog>
#include <QFileInfo>
#include <QJsonArray>
#include <QJsonDocument>
#include <QLabel>
#include <QLineEdit>
#include <QListWidget>
#include <QMenu>
#include <QMenuBar>
#include <QPlainTextEdit>
#include <QPushButton>
#include <QRegularExpression>
#include <QScreen>
#include <QScrollArea>
#include <QScrollBar>
#include <QSettings>
#include <QSet>
#include <QSpinBox>
#include <QSplitter>
#include <QStatusBar>
#include <QTemporaryDir>
#include <QTest>
#include <QTextBlock>
#include <QTextBrowser>
#include <QThread>
#include <QTimer>
#include <QToolButton>
#include <QTreeWidget>
#include <QUrl>

#include <cstdio>
#include <functional>
#include <memory>

namespace {
QString appearanceSource;
const QString folder = qEnvironmentVariable("HN_QA_DOCUMENTS");
const QString output = qEnvironmentVariable("HN_QA_OUTPUT");

bool until(const std::function<bool()> &condition, int timeout = 15000)
{
    QElapsedTimer timer;
    timer.start();
    do {
        QCoreApplication::processEvents();
        if (condition())
            return true;
        QTest::qWait(20);
    } while (timer.elapsed() < timeout);
    return condition();
}

bool saveNative(const QPixmap &pixmap, const QString &name)
{
    if (pixmap.isNull() || pixmap.width() < 40 || pixmap.height() < 14)
        return false;
    // Some macOS desktops restrict screen reads. A transparent or single-color
    // buffer does not count as a native visual capture.
    const QImage thumb = pixmap.toImage().scaled(60, 45, Qt::IgnoreAspectRatio, Qt::SmoothTransformation);
    QSet<QRgb> colors;
    for (int y = 0; y < thumb.height(); ++y)
        for (int x = 0; x < thumb.width(); ++x) {
            const auto pixel = thumb.pixel(x, y);
            if (qAlpha(pixel) > 32)
                colors.insert(pixel);
        }
    return colors.size() >= 8 && pixmap.save(QDir(output).filePath(name + ".png"));
}

QJsonObject matchData(QTreeWidgetItem &item)
{
    const auto data = item.data(0, Qt::UserRole);
    if (data.canConvert<QJsonObject>())
        return data.toJsonObject();
    return QJsonObject::fromVariantMap(data.toMap());
}

QByteArray frame(const QJsonObject &event)
{
    return QJsonDocument(event).toJson(QJsonDocument::Compact) + '\n';
}

QByteArray textHit(const QString &path, int line)
{
    const auto name = QFileInfo(path).fileName();
    return frame({{"type", "match"}, {"path", path}, {"displayPath", name}, {"kind", "text"},
        {"location", QJsonObject{{"index", line - 1}, {"line", line}}}, {"mode", "document"},
        {"text", "Match"}, {"start", 0}, {"length", 5}, {"blockStart", 0}, {"blockLength", 5}});
}

QByteArray searchDone(int hits, int files)
{
    return frame({{"type", "done"}, {"count", hits}, {"files", files}, {"errors", 0}, {"truncated", false}});
}

QString dialogContent(QDialog &dialog)
{
    QStringList pieces{dialog.windowTitle()};
    for (auto *w : dialog.findChildren<QLabel *>())
        pieces.append(w->text());
    for (auto *w : dialog.findChildren<QTextEdit *>())
        pieces.append(w->toPlainText());
    for (auto *w : dialog.findChildren<QPlainTextEdit *>())
        pieces.append(w->toPlainText());
    for (auto *w : dialog.findChildren<QListWidget *>())
        for (int i = 0; i < w->count(); ++i)
            pieces.append(w->item(i)->text());
    for (auto *w : dialog.findChildren<QTreeWidget *>())
        for (QTreeWidgetItemIterator it(w); *it; ++it)
            pieces.append((*it)->text(0));
    for (auto *w : dialog.findChildren<QComboBox *>())
        for (int i = 0; i < w->count(); ++i)
            pieces.append(w->itemText(i));
    return pieces.join('\n');
}
}

class BrowserCapture final : public QObject {
    Q_OBJECT
public:
    BrowserCapture()
    {
        for (const auto *scheme : {"https", "http"})
            QDesktopServices::setUrlHandler(scheme, this, "record");
    }
    ~BrowserCapture() override
    {
        for (const auto *scheme : {"https", "http"})
            QDesktopServices::unsetUrlHandler(scheme);
    }
    QList<QUrl> opened;
public slots:
    void record(const QUrl &url) { opened.append(url); }
};

#define VERIFY_OK(expression) do { const QString problem = (expression); \
    QVERIFY2(problem.isEmpty(), qPrintable(problem)); } while (false)

class GuiTests final : public QObject {
    Q_OBJECT
    std::unique_ptr<hn::MainWindow> window;
    QTreeWidget *results = nullptr;
    QLineEdit *query = nullptr;
    QTextBrowser *preview = nullptr;
    QAbstractButton *search = nullptr;
    QAbstractButton *cancel = nullptr;
    QJsonObject evidence;

    template<class T> T *control(const char *name) const
    {
        return window->findChild<T *>(name);
    }

    bool submit(const QString &text, bool keyboard = false, const QString &directory = folder)
    {
        control<QLineEdit>("folderEdit")->setText(directory);
        query->setText(text);
        if (!search->isEnabled())
            return false;
        if (keyboard) {
            query->setFocus();
            QTest::keyClick(query, Qt::Key_Return);
        } else {
            QTest::mouseClick(search, Qt::LeftButton);
        }
        QTest::qWait(30);
        return until([&] { return search->isEnabled() && !cancel->isEnabled(); });
    }

    QList<QTreeWidgetItem *> hits(const QString &filename) const
    {
        QList<QTreeWidgetItem *> matches;
        for (QTreeWidgetItemIterator it(results); *it; ++it) {
            const QJsonObject data = matchData(**it);
            if (data.contains("start") && QFileInfo(data.value("path").toString()).fileName() == filename)
                matches.append(*it);
        }
        return matches;
    }

    bool selectHit(const QString &file, int n = 0)
    {
        const auto items = hits(file);
        if (n < 0 || n >= items.size())
            return false;
        auto *item = items.at(n);
        if (item->parent())
            item->parent()->setExpanded(true);
        results->setCurrentItem(item);
        results->scrollToItem(item);
        return true;
    }

    QTextCursor selectedText(const QString &expected) const
    {
        if (preview->isVisible()) {
            for (const auto &selection : preview->extraSelections())
                if (selection.cursor.selectedText() == expected)
                    return selection.cursor;
        }
        return {};
    }

    void ensureOptions()
    {
        if (!control<QLineEdit>("pathPatternEdit")->isVisible())
            control<QAbstractButton>("optionsButton")->click();
    }

    QString modalText;
    QString captureModal(QAction *action, const QString &name)
    {
        if (!action)
            return "Missing action " + name;
        modalText.clear();
        QString error;
        QElapsedTimer timer;
        timer.start();
        QTimer observer;
        connect(&observer, &QTimer::timeout, this, [&] {
            auto *dialog = qobject_cast<QDialog *>(QApplication::activeModalWidget());
            if (!dialog) {
                for (auto *w : QApplication::topLevelWidgets())
                    if (w->isVisible() && (dialog = qobject_cast<QDialog *>(w)))
                        break;
            }
            if (dialog && dialog->isVisible()) {
                modalText = dialogContent(*dialog);
                error = guiqa::saveView(*dialog, name);
                dialog->reject();
                observer.stop();
            } else if (timer.elapsed() > 5000) {
                observer.stop();
            }
        });
        observer.start(50);
        action->trigger();
        until([&] { return !observer.isActive(); }, 5500);
        if (modalText.isEmpty())
            return "No visible " + name + " dialog";
        return error;
    }

private slots:
    void initTestCase()
    {
        const bool supplementary = qEnvironmentVariableIsSet("HN_QA_SUPPLEMENTARY") && QGuiApplication::platformName() == "offscreen";
        QVERIFY2(guiqa::hasExpectedPlatform() || supplementary, qPrintable("Expected operating system platform plugin, got " +
                                                         QGuiApplication::platformName()));
        QVERIFY(QFileInfo::exists(qEnvironmentVariable("HELLONEEDLE_WORKER")));
        QVERIFY(QFileInfo::exists(QDir(folder).filePath("report.pdf")));
        evidence = guiqa::platformDetails(appearanceSource);
        evidence["nativeAcceptance"] = !supplementary;
        const auto theme = qEnvironmentVariable("HN_QA_APPEARANCE");
        if (theme != "system")
            QCOMPARE(evidence["dark"].toBool(), theme == "dark");
    }

    void init()
    {
        QSettings().clear();
        window = std::make_unique<hn::MainWindow>();
        window->show();
        QVERIFY(QTest::qWaitForWindowExposed(window.get()));
        results = control<QTreeWidget>("resultsView");
        query = control<QLineEdit>("queryEdit");
        preview = control<QTextBrowser>("previewText");
        search = control<QAbstractButton>("searchButton");
        cancel = control<QAbstractButton>("cancelButton");
        QVERIFY(results && query && preview && search && cancel);
        QVERIFY(control<QLineEdit>("folderEdit") && control<QLineEdit>("pathPatternEdit"));
        QVERIFY(control<QAbstractButton>("optionsButton") && control<QComboBox>("modeCombo"));
    }

    void cleanup()
    {
        if (window)
            window->close();
        window.reset();
        QCoreApplication::sendPostedEvents(nullptr, QEvent::DeferredDelete);
    }

    void cleanupTestCase()
    {
        QFile file(QDir(output).filePath("appearance.json"));
        QVERIFY(file.open(QIODevice::WriteOnly));
        QVERIFY(file.write(QJsonDocument(evidence).toJson()) > 0);
    }

    void firstLaunch()
    {
        QVERIFY(window->windowTitle().contains("Hello Needle"));
        QCOMPARE(results->topLevelItemCount(), 0);
        QVERIFY(query->text().isEmpty());
        QJsonObject icon;
        const auto iconCheck = guiqa::verifyApplicationIcon(window->windowIcon(), icon);
        evidence["applicationIcon"] = icon;
        VERIFY_OK(iconCheck);
        auto *options = control<QToolButton>("optionsButton");
        QVERIFY(options);
        QVERIFY(options->isCheckable() && !options->isChecked());
        QJsonObject buttonIcons;
        VERIFY_OK(guiqa::inspectButtonIcons(*window, buttonIcons));
        evidence["buttonIcons"] = buttonIcons;
        QVERIFY(!options->accessibleName().isEmpty());
        const auto locale = qEnvironmentVariable("HN_QA_LANGUAGE");
        QString caption = search->text();
        caption.remove('&');
        if (locale != "system")
            QCOMPARE(caption, locale == "es" ? "Buscar" : "Search");
        else
            QVERIFY(caption == "Search" || caption == "Buscar");
        QCOMPARE(control<QComboBox>("modeCombo")->currentData().toString(), "document");
        const auto frame = window->frameGeometry(), available = window->screen()->availableGeometry();
        evidence["firstFrame"] = QJsonArray{frame.x(),frame.y(),frame.width(),frame.height()};
        evidence["searchCaption"] = caption;
        VERIFY_OK(guiqa::saveView(*window, "first-launch"));
        if (guiqa::hasExpectedPlatform())
            guiqa::captureFirstWindow(*window, evidence);
        QVERIFY2(frame.width() <= available.width() + 4 && frame.height() <= available.height() + 4,
                 qPrintable(QString("First launch %1x%2 exceeds work area %3x%4")
                     .arg(frame.width()).arg(frame.height()).arg(available.width()).arg(available.height())));
        VERIFY_OK(guiqa::inspectLayout(*window));
        QJsonObject colors, pixels, folderPixels, previewPixels;
        const QString colorError = guiqa::inspectColors(*window, colors);
        const QString pixelError = guiqa::inspectRenderedField(*query, pixels);
        const QString folderError = guiqa::inspectRenderedField(*control<QLineEdit>("folderEdit"), folderPixels);
        const QString previewError = guiqa::inspectPreviewPrompt(*preview, previewPixels);
        evidence["renderedFolderField"] = folderPixels;
        evidence["renderedPreviewPrompt"] = previewPixels;
        QJsonArray disabled;
        for (auto *button : {cancel, control<QAbstractButton>("openOriginalButton")})
            if (button && !button->isEnabled())
                disabled.append(button->objectName());
        evidence["disabledFirstLaunchControls"] = disabled;
        evidence["contrast"] = colors;
        evidence["renderedQueryField"] = pixels;
        VERIFY_OK(colorError);
        VERIFY_OK(pixelError);
        VERIFY_OK(folderError);
        VERIFY_OK(previewError);
    }

    void optionsAndMenus()
    {
        auto *button = control<QAbstractButton>("optionsButton");
        auto *pattern = control<QLineEdit>("pathPatternEdit");
        if (!pattern->isVisible()) {
            button->setFocus();
            QTest::keyClick(button, Qt::Key_Space);
        }
        QVERIFY(pattern->isVisible());
        QVERIFY(button->isChecked() && !button->icon().isNull());
        const auto available = window->screen()->availableGeometry();
        window->resize(std::min(760, available.width()-12), std::min(580, available.height()-48));
        QTest::qWait(35);
        VERIFY_OK(guiqa::saveView(*window, "options-narrow"));
        QJsonObject patternPixels;
        const auto pathError = guiqa::inspectRenderedField(*pattern, patternPixels);
        evidence["renderedPathFilter"] = patternPixels;
        VERIFY_OK(pathError);
        VERIFY_OK(guiqa::inspectLayout(*window));
        QVERIFY2(window->width() <= std::min(760, available.width()) + 4, "Expanded options prevent narrow resizing");

        auto *fileMenu = control<QMenu>("fileMenu");
        auto *help = control<QMenu>("helpMenu");
        QVERIFY(fileMenu && help && control<QAction>("aboutAction") && control<QAction>("licensesAction"));
        QVERIFY(!fileMenu->actions().isEmpty());
        // Cocoa presents the native menu bar outside the window. The system
        // screenshot and menu action/dialog tests cover the native actions there.
        evidence["nativeMenuBar"] = window->menuBar()->isNativeMenuBar();
#ifdef Q_OS_MACOS
        if (window->menuBar()->isNativeMenuBar()) {
            const auto titles = cocoaMenuTitles();
            evidence["cocoaMenuTitles"] = QJsonArray::fromStringList(titles);
            auto nativeTitle = [&](const char *name) {
                QString title = control<QMenu>(name)->title();
                return title.remove('&');
            };
            QJsonObject nativeActions;
            for (const auto *name : {"fileMenu", "searchMenu", "languageMenu", "helpMenu"}) {
                const QString title = nativeTitle(name);
                QVERIFY2(titles.contains(title), qPrintable("Missing native Cocoa menu: " + title));
                const QStringList actions = cocoaMenuActions(title);
                nativeActions[title] = QJsonArray::fromStringList(actions);
                QVERIFY2(!actions.isEmpty(), qPrintable("No reachable native Cocoa menu actions in " + title));
            }
            evidence["cocoaEnabledActions"] = nativeActions;
            if (qEnvironmentVariableIsSet("GITHUB_ACTIONS")) {
                QJsonObject owner;
                auto native = captureActiveCocoaMenu(window->winId(), owner);
                const bool captured = saveNative(native, "native-menu-bar");
                if (!captured && !owner.contains("reason"))
                    owner["reason"] = "The screen capture contained no readable menu pixels";
                evidence["cocoaMenuBarCapture"] = captured;
                evidence["cocoaMenuBarOwner"] = owner;
            }
        }
#endif
        if (!window->menuBar()->isNativeMenuBar()) {
            QJsonObject menuInk;
            const auto barInk = guiqa::inspectMenuInk(*window->menuBar(),
                window->menuBar()->actionGeometry(fileMenu->menuAction()), menuInk);
            evidence["renderedMenuBar"] = menuInk;
            VERIFY_OK(barInk);
            QJsonObject barItems;
            for (QAction *action : window->menuBar()->actions()) {
                if (!action->isEnabled() || !action->isVisible() || action->isSeparator())
                    continue;
                QJsonObject reading;
                const auto error = guiqa::inspectMenuInk(*window->menuBar(),
                    window->menuBar()->actionGeometry(action), reading);
                barItems[action->text()] = reading;
                VERIFY_OK(error);
            }
            evidence["renderedMenuBarItems"] = barItems;
            fileMenu->popup(window->menuBar()->mapToGlobal(QPoint(2, window->menuBar()->height())));
            QVERIFY(until([&] { return fileMenu->isVisible(); }, 3000));
            for (QAction *a : fileMenu->actions()) {
                if (a->isVisible() && !a->isSeparator()) {
                    QString label = a->text().section('\t', 0, 0);
                    label.remove('&');
                    QVERIFY2(fileMenu->actionGeometry(a).width() >= fileMenu->fontMetrics().horizontalAdvance(label),
                             qPrintable("Clipped menu action: " + label));
                }
            }
            const auto popupInk = guiqa::inspectMenuInk(*fileMenu,
                fileMenu->actionGeometry(control<QAction>("folderAction")), menuInk);
            evidence["renderedMenuItem"] = menuInk;
            VERIFY_OK(popupInk);
            VERIFY_OK(guiqa::saveView(*fileMenu, "file-menu"));
            QTest::keyClick(fileMenu, Qt::Key_Escape);
            QVERIFY(until([&] { return !fileMenu->isVisible(); }, 2000));
        }
        auto *box = control<QCheckBox>("caseCheck");
        QVERIFY(box);
        const bool previous = box->isChecked();
        box->setFocus();
        QTest::keyClick(box, Qt::Key_Space);
        QCOMPARE(box->isChecked(), !previous);
        query->setFocus();
        QTest::keyClick(query, Qt::Key_Tab);
        QVERIFY(QApplication::focusWidget() != query);
    }

    void compactWorkArea()
    {
        // The hidden panel has placeholder geometry. Translating and populating
        // it must not reserve height based on that width before the first show.
        QStatusBar bar;
        hn::SearchStatus panel(&bar);
        bar.addPermanentWidget(&panel, 1);
        const int initialMinimum = panel.minimumHeight();
        panel.retranslate();
        panel.setMessage(hn::ui("Ready to search."), 0);
        QCOMPARE(panel.minimumHeight(), initialMinimum);

        // At 150%, native Windows and Cocoa runners have 683×480 and 683×451
        // work areas respectively. Include the title bar in the measurements.
        for (int height : {480, 451}) {
            const QSize area(683, height);
            hn::MainWindow fresh;
            fresh.resize(622, qRound(area.height() * .85));
            fresh.show();
            QVERIFY(QTest::qWaitForWindowExposed(&fresh));
            const auto frame = fresh.frameGeometry();
            QVERIFY2(frame.width() <= area.width() + 4 && frame.height() <= area.height() + 4,
                     qPrintable(QString("Compact frame %1x%2 exceeds the 683x%3 work area")
                                .arg(frame.width()).arg(frame.height()).arg(height)));
            auto *split = fresh.findChild<QSplitter *>("resultSplitter");
            QVERIFY(split);
            QVERIFY2(split->orientation() == Qt::Horizontal,
                     qPrintable(QString("Expected side-by-side panes at %1x%2, minimum pane widths %3 + %4")
                         .arg(fresh.width()).arg(fresh.height()).arg(split->widget(0)->minimumSizeHint().width())
                         .arg(split->widget(1)->minimumSizeHint().width())));
            VERIFY_OK(guiqa::inspectLayout(fresh));
        }
    }

    void unicodeAndRepeat()
    {
        QVERIFY(submit("Needle", true));
        const auto items = hits("notes.txt");
        QCOMPARE(items.size(), 2);
        QVERIFY(selectHit("notes.txt", 1));
        QVERIFY(until([&] { return !selectedText("Needle").isNull(); }));
        const QTextCursor selected = selectedText("Needle");
        QVERIFY(selected.block().text().contains(QString::fromUtf8("🧵 Needle then Needle, café.")));
        QCOMPARE(selected.selectionStart() - selected.block().position(), selected.block().text().lastIndexOf("Needle"));
        QCOMPARE(control<QLabel>("previewTitle")->text(), hn::ui("Line %1").arg(3) + " - notes.txt");
        QVERIFY(control<QAbstractButton>("openOriginalButton")->isEnabled());
        VERIFY_OK(guiqa::saveView(*window, "search-results"));
        QVERIFY(submit("NeverPresentInFixture0204"));
        QCOMPARE(results->topLevelItemCount(), 0);
        QVERIFY(submit("Needle"));
        QCOMPARE(hits("notes.txt").size(), 2);
    }

    void statusBarLink()
    {
        auto *link = control<QAbstractButton>("crumbsLink");
        QVERIFY(link && link->isVisible());
        const bool spanish = hn::currentLanguage() == "es";
        QCOMPARE(link->text(), spanish ? "Lee Crumbs de Alfredo Llaquet-Alsina en TheCrumbs.org"
                                      : "Read Alfredo Llaquet-Alsina's Crumbs at TheCrumbs.org");
        QCOMPARE(link->toolTip(), spanish ? "Abrir TheCrumbs.org en tu navegador"
                                         : "Open TheCrumbs.org in your browser");
        QEvent leave(QEvent::Leave);
        QCoreApplication::sendEvent(link, &leave);
        QVERIFY(!link->font().underline() && !link->font().bold());
        QVERIFY2(link->heightForWidth(link->sizeHint().width()) <= link->fontMetrics().height() + 6,
                 "The domain wraps onto another line even at the link's full preferred width");

        BrowserCapture browser;
        const QPoint center = link->rect().center();
        const int halfText = link->fontMetrics().horizontalAdvance(link->text()) / 2;
        for (const int x : {center.x() - halfText + 2, center.x(), center.x() + halfText - 2}) {
            const QPoint point(x, center.y());
            QVERIFY(link->rect().contains(point));
            QEnterEvent enter(point, link->mapTo(window.get(), point), link->mapToGlobal(point));
            QCoreApplication::sendEvent(link, &enter);
            QVERIFY(link->font().underline());
            QTest::mouseClick(link, Qt::LeftButton, Qt::NoModifier, point);
            QCoreApplication::sendEvent(link, &leave);
            QVERIFY(!link->font().underline());
        }
        QVERIFY(link->focusPolicy() & Qt::TabFocus);
        link->setFocus(Qt::TabFocusReason);
        QTest::keyClick(link, Qt::Key_Space);
        QCOMPARE(browser.opened.size(), 4);
        for (const QUrl &url : browser.opened)
            QCOMPARE(url.adjusted(QUrl::StripTrailingSlash), QUrl("https://www.thecrumbs.org"));
    }

    void scannedFilesAndErrors()
    {
        QTemporaryDir docs;
        QVERIFY(docs.isValid());
        const QString title = "Witness statements in the appeal against gravity and supplementary exhibits from the hearing.docx";
        QVERIFY(QFile::copy(QDir(folder).filePath("letter.docx"), docs.filePath(title)));
        QFile quiet(docs.filePath("01-quiet.txt"));
        QVERIFY(quiet.open(QIODevice::WriteOnly));
        QCOMPARE(quiet.write("No hits here."), 13);
        quiet.close();
        QFile unreadable(docs.filePath("02-invalid.txt"));
        QVERIFY(unreadable.open(QIODevice::WriteOnly));
        QCOMPARE(unreadable.write(QByteArray(8, '\0')), 8);
        unreadable.close();

        QFile video(docs.filePath("03-movie.mp4"));
        QVERIFY(video.open(QIODevice::WriteOnly));
        video.write("Needle");
        video.close();

        auto *status = control<QLabel>("statusLabel");
        auto *errors = control<QLabel>("errorCountLabel");
        auto *details = control<QAbstractButton>("issuesButton");
        QVERIFY(status && errors && details);
        QVERIFY(!errors->isVisible() && !details->isVisible());
        const bool spanish = hn::currentLanguage() == "es";
        const QString zero = spanish ? "0 archivos examinados" : "0 files scanned";
        bool countedBeforeFirstHit = false;
        QObject observation;
        connect(results->model(), &QAbstractItemModel::rowsInserted, &observation, [&] {
            if (cancel->isEnabled() && !status->text().contains(zero))
                countedBeforeFirstHit = true;
        });
        QVERIFY(submit("Needle", false, docs.path()));
        QVERIFY(countedBeforeFirstHit);
        const QString scanned = spanish ? "4 archivos examinados" : "4 files scanned";
        QVERIFY2(status->text().contains(scanned), qPrintable(status->text()));
        QVERIFY(status->text().contains(spanish ? "1 coincidencia" : "1 match"));
        QCOMPARE(errors->text(), "1 error");
        QVERIFY(errors->isVisible() && details->isVisible());
        VERIFY_OK(guiqa::inspectLayout(*window));

        window->resize(qMin(514, window->width()), window->height());
        QVERIFY(selectHit(title));
        QVERIFY(until([&] { return !selectedText("Needle").isNull(); }));
        auto *heading = control<QLabel>("previewTitle");
        auto *open = control<QAbstractButton>("openOriginalButton");
        QVERIFY(heading && open && open->isEnabled());
        QCOMPARE(heading->text(), (spanish ? QString("Párrafo 2 - ") : QString("Paragraph 2 - ")) + title);
        QVERIFY(heading->wordWrap() && heading->alignment().testFlag(Qt::AlignLeft));
        QVERIFY(heading->height() >= 2 * heading->fontMetrics().height() - 3);
        const auto headingStart = heading->mapTo(window.get(), QPoint());
        const auto buttonStart = open->mapTo(window.get(), QPoint());
        QVERIFY(std::abs(headingStart.x() - buttonStart.x()) <= 8);
        QVERIFY(buttonStart.y() >= headingStart.y() + heading->height() - 3);
        VERIFY_OK(guiqa::inspectLayout(*window));
        VERIFY_OK(guiqa::saveView(*window, "progress-and-errors-narrow"));

        QVERIFY(unreadable.remove());
        QVERIFY(submit("NoMatchAnywhere", false, docs.path()));
        QVERIFY(status->text().contains(spanish ? "3 archivos examinados" : "3 files scanned"));
        QVERIFY(status->text().contains(spanish ? "0 coincidencias" : "0 matches"));
        QVERIFY(!errors->isVisible() && !details->isVisible());
    }

    void wordAndOfficePreviews()
    {
        QVERIFY(submit("Needle"));
        const QStringList docs = {"letter.docx", "letter.odt", "letter.fodt", "letter.rtf", "sample.doc"};
        for (const auto &name : docs) {
            QVERIFY2(selectHit(name), qPrintable("Missing hit in " + name));
            QVERIFY2(until([&] { return !selectedText("Needle").isNull() &&
                       preview->toPlainText().contains("Before the passage.") &&
                       preview->toPlainText().contains("After the passage."); }), qPrintable(name));
            if (name == "letter.odt")
                VERIFY_OK(guiqa::saveView(*window, "word-preview"));
            if (name == "sample.doc")
                evidence["legacyWordGuiTested"] = true;
            // Force an intervening state to prevent a previous file's identical
            // text satisfying the assertion while its replacement is still loading.
            QVERIFY(selectHit("notes.txt"));
            QVERIFY(until([&] { return preview->toPlainText().contains("Second line"); }));
        }
        for (const auto &name : {QString("sheet.xlsx"), QString("sheet.ods")}) {
            QVERIFY2(selectHit(name), qPrintable(name));
            QVERIFY(until([&] { return !selectedText("Needle").isNull() && preview->toPlainText().contains("Needle budget"); }));
            const auto position = matchData(*results->currentItem())["location"].toObject();
            QCOMPARE(position["cell"].toString(), "C7");
            QCOMPARE(position["sheet"].toString(), "Quarterly");
            QCOMPARE(control<QLabel>("previewTitle")->text(), QStringLiteral("Quarterly · C7 - ") + name);
        }
        for (const auto &name : {QString("talk.pptx"), QString("talk.odp")}) {
            QVERIFY2(selectHit(name), qPrintable(name));
            QVERIFY(until([&] { return !selectedText("Needle").isNull() && preview->toPlainText().contains("Needle on stage."); }));
            QCOMPARE(matchData(*results->currentItem())["location"].toObject()["slide"].toInt(), 2);
        }
    }

    void markupModes()
    {
        QVERIFY(!preview->openExternalLinks() && !preview->openLinks());
        auto *mode = control<QComboBox>("modeCombo");
        QVERIFY(submit("The Latest Batch"));
        for (const auto &name : {QString("page.html"), QString("readme.md")}) {
            QVERIFY2(selectHit(name), qPrintable(name));
            QVERIFY(until([&] { return !selectedText("The Latest Batch").isNull(); }));
            QVERIFY(!preview->toPlainText().contains("forbiddenScriptMarker"));
            QVERIFY(!preview->toPlainText().contains("**Latest**"));
            QTextCursor word = preview->document()->find("Latest");
            QVERIFY(!word.isNull());
            QVERIFY2(word.charFormat().fontWeight() >= QFont::DemiBold, "Inline bold lost in document preview");
        }
        ensureOptions();
        const int source = mode->findData("source");
        QVERIFY(source >= 0);
        mode->setCurrentIndex(source);
        QVERIFY(submit("<strong>"));
        QVERIFY(selectHit("page.html"));
        QVERIFY(until([&] { return !selectedText("<strong>").isNull(); }));
        QVERIFY(preview->toPlainText().contains("</strong>"));
        QVERIFY(submit("**Latest**"));
        QVERIFY(selectHit("readme.md"));
        QVERIFY(until([&] { return !selectedText("**Latest**").isNull(); }));
    }

    void pdfNavigation()
    {
        auto *image = control<QLabel>("pdfImage");
        auto *label = control<QLabel>("pdfPageLabel");
        auto *prev = control<QAbstractButton>("previousPageButton");
        auto *next = control<QAbstractButton>("nextPageButton");
        auto *zoom = control<QAbstractButton>("zoomInButton");
        auto *fit = control<QAbstractButton>("fitPageButton");
        QVERIFY(image && label && prev && next && zoom && fit);
        QVERIFY(submit("Needle"));
        QCOMPARE(hits("report.pdf").size(), 2);
        QVERIFY(selectHit("report.pdf", 0));
        auto ready = [&] {
            return image->isVisible() && !image->pixmap().isNull() && label->text().contains('2');
        };
        QVERIFY(until(ready));
        QCOMPARE(control<QLabel>("previewTitle")->text(), hn::ui("Page %1").arg(2) + " - report.pdf");
        const auto first = image->pixmap().toImage();
        QVERIFY(selectHit("report.pdf", 1));
        QVERIFY(until([&] { return ready() && image->pixmap().toImage() != first; }));
        VERIFY_OK(guiqa::saveView(*window, "pdf-preview"));
        auto *toggle = control<QAbstractButton>("pdfTextButton");
        QVERIFY(toggle);
        QTest::mouseClick(toggle, Qt::LeftButton);
        QVERIFY(until([&] { return preview->isVisible()
            && preview->toPlainText().contains("This is the second line on this page.")
            && !selectedText("Needle").isNull(); }));
        QTest::mouseClick(toggle, Qt::LeftButton);
        QVERIFY(until(ready));
        const QSize original = image->pixmap().size();
        QTest::mouseClick(zoom, Qt::LeftButton);
        QVERIFY(until([&] { return image->pixmap().width() > original.width(); }));
        auto *scroller = control<QWidget>("pdfPane")->findChild<QScrollArea *>();
        QVERIFY(scroller);
        for (int i = 0; i < 12 && image->height() < 2.4 * scroller->viewport()->height(); ++i)
            QTest::mouseClick(zoom, Qt::LeftButton);
        auto *bar = scroller->verticalScrollBar();
        QVERIFY(bar->maximum() > 0);
        // Locate the actual amber annotation independently of the worker's
        // coordinates, including when the pixmap uses a high device pixel ratio.
        const auto raster = image->pixmap().toImage().convertToFormat(QImage::Format_RGB32);
        int top = raster.height(), bottom = -1;
        for (int y = 0; y < raster.height(); y += 2) {
            const auto *pixels = reinterpret_cast<const QRgb *>(raster.constScanLine(y));
            for (int x = 0; x < raster.width(); x += 2) {
                const auto c = pixels[x];
                if (qRed(c) > 210 && qRed(c) - qGreen(c) > 8 && qGreen(c) - qBlue(c) > 24) {
                    top = qMin(top, y);
                    bottom = y;
                    break;
                }
            }
        }
        QVERIFY2(bottom >= top, "Missing painted PDF match after zooming");
        const int hitY = qRound((top + bottom) * 0.5 * image->height() / raster.height());
        // Showing a scrollbar after zoom changes the viewport midpoint on some desktops.
        auto *viewport = scroller->viewport();
        const auto verticalOffset = [&] {
            return image->mapTo(viewport, QPoint(0, hitY)).y() - viewport->rect().center().y();
        };
        QVERIFY2(until([&] { return std::abs(verticalOffset()) <= 4; }),
                 qPrintable(QString("Painted PDF match offset from viewport center: %1").arg(verticalOffset())));
        const int target = hitY - viewport->rect().center().y();
        QVERIFY2(target > bar->minimum() + 3 && target < bar->maximum() - 3,
                 "The example must allow centering without clamping at a page edge");
        VERIFY_OK(guiqa::saveView(*window, "pdf-zoomed-centered"));
        bar->setValue(bar->minimum());
        QTest::qWait(150);
        QCOMPARE(bar->value(), bar->minimum());
        auto *tile = control<QLabel>("pdfDetailRaster");
        QVERIFY(tile);
        for (int i = 0; i < 20 && qMax(image->width(), image->height()) * image->devicePixelRatioF() <= 1850; ++i)
            QTest::mouseClick(zoom, Qt::LeftButton);
        QVERIFY(until([&] { return tile->isVisible() && !tile->pixmap().isNull(); }));
        QVERIFY(tile->width() < image->width() || tile->height() < image->height());
        QTest::mouseClick(fit, Qt::LeftButton);
        QVERIFY(until([&] { return image->pixmap().width() <= original.width() + 2; }));
        QTest::mouseClick(prev, Qt::LeftButton);
        QVERIFY(until([&] { return !prev->isEnabled() && label->text().contains('1'); }));
        QTest::mouseClick(next, Qt::LeftButton);
        QVERIFY(until([&] { return prev->isEnabled() && label->text().contains('2'); }));
        QTest::mouseClick(next, Qt::LeftButton);
        QVERIFY(until([&] { return !next->isEnabled() && label->text().contains('3'); }));
        QVERIFY(toggle->isVisible());
        QTest::mouseClick(toggle, Qt::LeftButton);
        QVERIFY(until([&] { return preview->isVisible() && preview->textCursor().block().text().contains("Closing page"); }));
        QTest::mouseClick(toggle, Qt::LeftButton);
        QVERIFY(until([&] { return image->isVisible() && !image->pixmap().isNull() && !next->isEnabled(); }));
    }

    void resultNavigation()
    {
        const QStringList identifiers{"nextDocumentButton", "nextMatchButton", "previousMatchButton", "previousDocumentButton"};
        const bool spanish = hn::currentLanguage() == "es";
        const QStringList names = spanish
            ? QStringList{"Documento siguiente", "Coincidencia siguiente", "Coincidencia anterior", "Documento anterior"}
            : QStringList{"Next document", "Next match", "Previous match", "Previous document"};
#ifdef Q_OS_MACOS
        const auto physicalControl = Qt::MetaModifier;
#else
        const auto physicalControl = Qt::ControlModifier;
#endif
        const QList<QList<QKeySequence>> keys{
            {QKeySequence(physicalControl | Qt::Key_Down)}, {QKeySequence(Qt::Key_F3), QKeySequence(Qt::ALT | Qt::Key_Down)},
            {QKeySequence(Qt::SHIFT | Qt::Key_F3), QKeySequence(Qt::ALT | Qt::Key_Up)}, {QKeySequence(physicalControl | Qt::Key_Up)}};
        QList<QToolButton *> buttons;
        int previousX = -1;
        for (int i = 0; i < identifiers.size(); ++i) {
            auto *button = window->findChild<QToolButton *>(identifiers[i]);
            QVERIFY(button && button->isVisible() && !button->isEnabled());
            QVERIFY(!button->icon().isNull() && button->toolButtonStyle() == Qt::ToolButtonIconOnly);
            QCOMPARE(button->accessibleName(), names[i]);
            QVERIFY2(button->toolTip().contains(names[i]), qPrintable(button->toolTip()));
            for (const auto &key : keys[i])
                QVERIFY2(button->toolTip().contains(key.toString(QKeySequence::NativeText)), qPrintable(button->toolTip()));
            const auto position = button->mapTo(window.get(), QPoint());
            QVERIFY(position.x() > previousX && position.y() < results->mapTo(window.get(), QPoint()).y());
            previousX = position.x();
            buttons << button;
        }

        // Simulate a worker finishing zulu before alpha, then another zulu
        // occurrence. Every selection invokes the installed worker for its preview.
        guiqa::SearchStream stream;
        const QString alpha = stream.folder.filePath("alpha.txt"), zulu = stream.folder.filePath("zulu.txt");
        for (const auto &path : {alpha, zulu}) {
            QFile file(path);
            QVERIFY(file.open(QIODevice::WriteOnly));
            file.write("Match one in " + QFileInfo(path).fileName().toUtf8() + "\nMatch two in "
                       + QFileInfo(path).fileName().toUtf8() + "\n");
        }
        QVERIFY(stream.prepare(textHit(zulu, 1), textHit(alpha, 1) + textHit(alpha, 2) + textHit(zulu, 2) + searchDone(4, 2)));
        control<QLineEdit>("folderEdit")->setText(stream.folder.path());
        query->setText("Match");
        stream.start(*search);
        auto selected = [&](const QString &name, int line) {
            auto *item = results->currentItem();
            if (!item || !item->isSelected() || !item->parent() || !item->parent()->isExpanded()
                || !results->viewport()->rect().intersects(results->visualItemRect(item))) return false;
            const auto data = matchData(*item);
            const auto cursor = selectedText("Match");
            return QFileInfo(data["path"].toString()).fileName() == name && data["location"].toObject()["line"].toInt() == line
                && !cursor.isNull() && cursor.block().text() == "Match " + (line == 1 ? QStringLiteral("one in ") : QStringLiteral("two in ")) + name;
        };
        QVERIFY(until([&] { return selected("zulu.txt", 1); }));
        QVERIFY(cancel->isEnabled());
        for (auto *button : buttons) QVERIFY(button->isEnabled());
        QTest::mouseClick(buttons[0], Qt::LeftButton);
        QVERIFY(selected("zulu.txt", 1));
        const QJsonObject originalHit = matchData(*results->currentItem());
        QVERIFY(stream.resume());
        QVERIFY(until([&] { return !cancel->isEnabled(); }));
        QCOMPARE(matchData(*results->currentItem()), originalHit);
        QCOMPARE(results->topLevelItem(0)->text(0), QStringLiteral("alpha.txt"));
        QCOMPARE(results->topLevelItem(1)->text(0), QStringLiteral("zulu.txt"));
        results->topLevelItem(0)->setExpanded(false);

        query->setFocus();
        QTest::keyClick(query, Qt::Key_Down, physicalControl);
        QVERIFY(until([&] { return selected("alpha.txt", 1); }));
        QTest::keyClick(query, Qt::Key_F3);
        QVERIFY(until([&] { return selected("alpha.txt", 2); }));
        QTest::keyClick(query, Qt::Key_Down, Qt::AltModifier);
        QVERIFY(until([&] { return selected("zulu.txt", 1); }));
        QTest::keyClick(query, Qt::Key_F3, Qt::ShiftModifier);
        QVERIFY(until([&] { return selected("alpha.txt", 2); }));
        QTest::keyClick(query, Qt::Key_Up, Qt::AltModifier);
        QVERIFY(until([&] { return selected("alpha.txt", 1); }));
        QTest::mouseClick(buttons[2], Qt::LeftButton);
        QVERIFY(until([&] { return selected("zulu.txt", 2); }));
        QTest::mouseClick(buttons[1], Qt::LeftButton);
        QVERIFY(until([&] { return selected("alpha.txt", 1); }));
        QTest::mouseClick(buttons[3], Qt::LeftButton);
        QVERIFY(until([&] { return selected("zulu.txt", 1); }));
        preview->setFocus();
        QTest::keyClick(preview, Qt::Key_Up, physicalControl);
        QVERIFY(until([&] { return selected("alpha.txt", 1); }));
        results->setCurrentItem(results->topLevelItem(1));
        QTest::mouseClick(buttons[1], Qt::LeftButton);
        QVERIFY(until([&] { return selected("zulu.txt", 1); }));
        results->setCurrentItem(results->topLevelItem(1));
        QTest::mouseClick(buttons[2], Qt::LeftButton);
        QVERIFY(until([&] { return selected("alpha.txt", 2); }));
        VERIFY_OK(guiqa::saveView(*window, "result-navigation"));

        // A provisional result can be withdrawn when the decoder reaches the end.
        guiqa::SearchStream retracted;
        const auto path = retracted.folder.filePath("zulu.txt");
        QVERIFY(QFile::copy(zulu, path));
        QVERIFY(retracted.prepare(textHit(path, 1), frame({{"type", "discard"}, {"path", path}}) + searchDone(0, 1)));
        control<QLineEdit>("folderEdit")->setText(retracted.folder.path());
        retracted.start(*search);
        QVERIFY(until([&] { return selected("zulu.txt", 1); }));
        QVERIFY(retracted.resume());
        QVERIFY(until([&] { return search->isEnabled(); }));
        QCOMPARE(results->topLevelItemCount(), 0);
        QVERIFY(results->currentItem() == nullptr && preview->extraSelections().isEmpty());
        for (auto *button : buttons) QVERIFY(!button->isEnabled());
    }

    void previewCentering()
    {
        QTemporaryDir docs;
        QFile file(docs.filePath("lines.txt"));
        QVERIFY(file.open(QIODevice::WriteOnly));
        const auto contextLine = QByteArray("Some context for the preview. ").repeated(18) + '\n';
        file.write("Match on the first line.\n");
        for (int n = 0; n < 90; ++n) file.write(contextLine);
        file.write("Match on the last line.");
        file.close();
        QVERIFY(submit("Match", false, docs.path()));
        auto centered = [&](const QString &line) {
            const auto highlight = selectedText("Match");
            if (highlight.isNull() || highlight.block().text() != line) return false;
            auto start = highlight;
            start.setPosition(highlight.selectionStart());
            const auto rect = preview->cursorRect(start);
            return std::abs(rect.center().y() - preview->viewport()->rect().center().y()) <= qMax(3, rect.height() / 2);
        };
        QVERIFY(selectHit("lines.txt", 0));
        QVERIFY(until([&] { return centered("Match on the first line."); }));
        QVERIFY(selectHit("lines.txt", 1));
        QVERIFY(until([&] { return centered("Match on the last line."); }));
        VERIFY_OK(guiqa::saveView(*window, "centered-final-line"));
        auto *bar = preview->verticalScrollBar();
        QVERIFY(bar->value() > bar->minimum());
        bar->setValue(bar->minimum());
        QTest::qWait(150);
        QCOMPARE(bar->value(), bar->minimum());

        auto *split = window->findChild<QSplitter *>();
        auto *heading = control<QLabel>("previewTitle");
        QVERIFY(split && heading);
        for (const auto orientation : {Qt::Horizontal, Qt::Vertical}) {
            window->resize(orientation == Qt::Horizontal ? 760 : 530, window->height());
            QVERIFY(until([&] { return split->orientation() == orientation; }));
            auto *handle = split->handle(1);
            const QPoint relative = heading->mapTo(split, QPoint()) - handle->mapTo(split, QPoint());
            const int gap = orientation == Qt::Horizontal ? relative.x() - handle->width() : relative.y() - handle->height();
            QVERIFY2(std::abs(gap - 24) <= 1, qPrintable(QStringLiteral("Preview content gap at splitter: %1").arg(gap)));
        }
    }

    void documentViewsAndScope()
    {
        QTemporaryDir docs;
        QVERIFY(docs.isValid());
        QDir root(docs.path());
        QVERIFY(root.mkpath("A/inside") && root.mkpath("B"));
        const QStringList paths{"A/inside/notes.txt", "B/notes.txt", "z.txt"};
        for (const auto &relative : paths) {
            QFile file(root.filePath(relative));
            QVERIFY(file.open(QIODevice::WriteOnly));
            file.write("Match here\n");
        }
        auto *view = control<QComboBox>("resultViewCombo");
        auto *directory = control<QPlainTextEdit>("previewDirectory");
        auto *scope = control<QCheckBox>("excludeSubdirectoriesCheck");
        QVERIFY(view && directory && scope);
        QCOMPARE(view->currentData().toString(), QStringLiteral("names"));
        QVERIFY(!scope->isChecked());
        QVERIFY(submit("Match", false, root.path()));
        QCOMPARE(results->topLevelItemCount(), 3);
        QCOMPARE(results->topLevelItem(0)->text(0), QStringLiteral("notes.txt"));
        QCOMPARE(results->topLevelItem(1)->text(0), QStringLiteral("notes.txt"));
        QCOMPARE(results->topLevelItem(2)->text(0), QStringLiteral("z.txt"));
        QVERIFY(selectHit("notes.txt", 0));
        const QString target = root.filePath(paths.first());
        const QString parent = QDir::toNativeSeparators(QFileInfo(target).absolutePath());
        QVERIFY(until([&] { return directory->isVisible() && directory->toPlainText().endsWith(parent); }));
        auto *caption = control<QLabel>("previewDirectoryLabel");
        QVERIFY(caption && caption->isVisible());
        QCOMPARE(caption->text(), hn::ui("Directory:"));
        QVERIFY(directory->textInteractionFlags().testFlag(Qt::TextSelectableByMouse));
        directory->selectAll();
        directory->copy();
        QCOMPARE(QApplication::clipboard()->text(), parent);
        QVERIFY(until([&] { return !selectedText("Match").isNull(); }));
        const auto hit = matchData(*results->currentItem());
        QCOMPARE(QDir::fromNativeSeparators(hit.value("path").toString()), target);
        for (const auto &mode : {QStringLiteral("paths"), QStringLiteral("tree"), QStringLiteral("names")}) {
            const int index = view->findData(mode);
            QVERIFY(index >= 0);
            view->setCurrentIndex(index);
            QVERIFY(until([&] { return results->currentItem() && matchData(*results->currentItem()) == hit; }));
            QVERIFY(directory->toPlainText().endsWith(parent));
            QVERIFY(!selectedText("Match").isNull());
            auto *file = results->currentItem()->parent();
            QVERIFY(file);
            if (mode == "tree") {
                QCOMPARE(file->text(0), QStringLiteral("notes.txt"));
                QVERIFY(file->parent() && file->parent()->parent());
                QCOMPARE(file->parent()->text(0), QStringLiteral("inside"));
                QCOMPARE(file->parent()->parent()->text(0), QStringLiteral("A"));
                QVERIFY(file->parent()->isExpanded());
                VERIFY_OK(guiqa::saveView(*window, "folder-tree-and-directory"));
            } else if (mode == "paths") {
                QCOMPARE(QDir::fromNativeSeparators(file->text(0)), paths.first());
            }
        }
        ensureOptions();
        scope->setChecked(true);
        view->setCurrentIndex(view->findData("tree"));
        QVERIFY(submit("Match", false, root.path()));
        QCOMPARE(results->topLevelItemCount(), 1);
        QCOMPARE(results->topLevelItem(0)->text(0), QStringLiteral("z.txt"));
        QVERIFY(hits("notes.txt").isEmpty());
        QVERIFY(until([&] { return directory->toPlainText().endsWith(QDir::toNativeSeparators(root.path())); }));
        const QString longQuery = QString(32767, 'x') + "tail";
        query->setText(longQuery);
        QCOMPARE(query->text(), longQuery);
        auto *pattern = control<QLineEdit>("pathPatternEdit");
        pattern->setText(longQuery);
        QCOMPARE(pattern->text(), longQuery);
        const qint64 longLine = Q_INT64_C(2147483648);
        QCOMPARE(hn::locationLabel({{"line",longLine}}, {}), hn::ui("Line %1").arg(longLine));
        window->close();
        hn::MainWindow reopened;
        QCOMPARE(reopened.findChild<QComboBox *>("resultViewCombo")->currentData().toString(), QStringLiteral("tree"));
        QVERIFY(reopened.findChild<QCheckBox *>("excludeSubdirectoriesCheck")->isChecked());
    }

    void allErrorDetails()
    {
        guiqa::SearchStream stream;
        QByteArray diagnostics;
        for (int n = 1; n <= 490; ++n) {
            diagnostics += frame({{"type","error"}, {"code","read"},
                {"path",stream.folder.filePath(QStringLiteral("file-%1.txt").arg(n))},
                {"message",QStringLiteral("Diagnostic number %1.").arg(n)}});
        }
        QVERIFY(stream.prepare({}, diagnostics + frame({{"type","done"},{"files",490},{"count",0},{"errors",490}})));
        control<QLineEdit>("folderEdit")->setText(stream.folder.path());
        query->setText("Match");
        stream.start(*search);
        QVERIFY(stream.resume());
        QVERIFY(until([&] { return search->isEnabled(); }));
        auto *details = control<QAbstractButton>("issuesButton");
        QVERIFY(details && details->isVisible());
        QStringList collected;
        bool copyable = false;
        QTimer::singleShot(0, this, [&] {
            auto *dialog = qobject_cast<QDialog *>(QApplication::activeModalWidget());
            if (!dialog) return;
            auto *body = dialog->findChild<QPlainTextEdit *>("errorDetailsText");
            auto *next = dialog->findChild<QAbstractButton *>("nextErrorsButton");
            if (body && next) {
                for (int p = 0; p < 490; ++p) {
                    collected << body->toPlainText();
                    if (!next->isEnabled()) break;
                    next->click();
                }
                auto cursor = body->document()->find("Diagnostic number 490.");
                if (!cursor.isNull()) {
                    body->setTextCursor(cursor);
                    body->copy();
                    copyable = QApplication::clipboard()->text() == "Diagnostic number 490.";
                }
            }
            dialog->accept();
        });
        details->click();
        const QString all = collected.join('\n');
        for (int n = 1; n <= 490; ++n) {
            QCOMPARE(all.count(QStringLiteral("file-%1.txt").arg(n)), 1);
            QCOMPARE(all.count(QStringLiteral("Diagnostic number %1.").arg(n)), 1);
        }
        QVERIFY(copyable);
    }

    void copyLongMatch()
    {
        QTemporaryDir documents;
        QFile file(documents.filePath("large.txt"));
        QVERIFY(file.open(QIODevice::WriteOnly));
        const QString expected = QStringLiteral("Start") + QString(145000, 'x') + QStringLiteral("🧵End");
        file.write("prefix " + expected.toUtf8() + " suffix");
        file.close();
        ensureOptions();
        control<QCheckBox>("regexCheck")->setChecked(true);
        QVERIFY(submit(QStringLiteral("Startx+🧵End"), false, documents.path()));
        QVERIFY(selectHit("large.txt"));
        auto *nextPart = control<QAbstractButton>("nextPreviewPart");
        auto *copy = control<QAbstractButton>("copyMatchButton");
        QVERIFY(nextPart && copy);
        QVERIFY(until([&] { return nextPart->isVisible() && nextPart->isEnabled(); }));
        nextPart->click();
        QVERIFY(until([&] { return preview->textCursor().selectedText().endsWith(QStringLiteral("🧵End")); }));
        copy->click();
        QVERIFY(until([&] { return QApplication::clipboard()->text() == expected; }, 15000));
    }

    void bulkResultNavigation()
    {
        QTemporaryDir docs;
        QVERIFY(docs.isValid());
        QFile first(docs.filePath("a.txt")), last(docs.filePath("z.txt"));
        QVERIFY(first.open(QIODevice::WriteOnly) && last.open(QIODevice::WriteOnly));
        QCOMPARE(first.write(QByteArray("Needle\n").repeated(100000)), qint64(700000));
        last.write("Needle at the end\n");
        first.close();
        last.close();
        control<QLineEdit>("folderEdit")->setText(docs.path());
        query->setText("Needle");
        search->click();
        QVERIFY(until([&] { return search->isEnabled(); }, 100000));
        auto *status = control<QLabel>("statusLabel");
        QVERIFY2(status->text().contains("100001"), qPrintable(status->text()));
        // Collapsed file groups must never allocate one Qt item per occurrence.
        int residentRows = 0;
        for (QTreeWidgetItemIterator it(results); *it; ++it) ++residentRows;
        QVERIFY(residentRows < 1000);
        QVERIFY(selectHit("a.txt", 0));
        control<QToolButton>("previousMatchButton")->click();
        QVERIFY(until([&] { return !selectedText("Needle").isNull()
                    && preview->toPlainText().contains("at the end"); }));
        control<QToolButton>("previousMatchButton")->click();
        QVERIFY(until([&] { auto *item = results->currentItem();
            return item && matchData(*item)["location"].toObject()["line"].toInteger() == 100000
                && !selectedText("Needle").isNull(); }));
    }

    void documentPageBoundaries()
    {
        guiqa::SearchStream stream;
        QVERIFY(stream.folder.isValid());
        auto name = [](int n) { return QStringLiteral("d%1-Match.txt").arg(n, 3, 10, QLatin1Char('0')); };
        auto writeDocument = [&](const QString &fileName) {
            QFile file(stream.folder.filePath(fileName));
            return file.open(QIODevice::WriteOnly) && file.write("Match\n") == 6;
        };
        auto selectedName = [&] {
            auto *item = results->currentItem();
            return item ? QFileInfo(matchData(*item)["path"].toString()).fileName() : QString();
        };
        QByteArray initial;
        for (int n = 0; n < 130; ++n) {
            QVERIFY(writeDocument(name(n)));
            const QString path = stream.folder.filePath(name(n));
            if (n == 10) {
                initial += frame({{"type", "match"}, {"path", path}, {"displayPath", name(n)},
                    {"kind", "filename"}, {"text", name(n)}, {"start", 5}, {"length", 5},
                    {"blockStart", 5}, {"blockLength", 5}, {"location", QJsonObject{{"index", 0}, {"filename", true}}}});
            }
            initial += textHit(path, 1);
        }
        const QString earlier = QStringLiteral("a-Match.txt");
        QVERIFY(writeDocument(earlier));
        const QByteArray rest = textHit(stream.folder.filePath(earlier), 1)
            + frame({{"type", "discard"}, {"path", stream.folder.filePath(name(10))}})
            + searchDone(131, 131);
        QVERIFY(stream.prepare(initial, rest));
        auto *views = control<QComboBox>("resultViewCombo");
        views->setCurrentIndex(views->findData("names"));
        control<QLineEdit>("folderEdit")->setText(stream.folder.path());
        query->setText("Match");
        stream.start(*search);
        auto *range = control<QLabel>("filesPageLabel");
        auto *back = control<QPushButton>("previousFilesPageButton");
        auto *next = control<QPushButton>("nextFilesPageButton");
        QVERIFY(range && back && next);
        auto page = [&](int first, int last, int total) {
            return range->text() == hn::ui("Files %1–%2 of %3").arg(first).arg(last).arg(total);
        };
        QVERIFY(until([&] { return page(1, 64, 130); }));
        QVERIFY(selectHit(name(63)));
        // Inserting an earlier file moves the selected document onto page two.
        // Withdrawing another file's content keeps its filename and must preserve that pending move.
        QVERIFY(stream.resume());
        QVERIFY(until([&] { return search->isEnabled() && page(65, 128, 131); }));
        QCOMPARE(selectedName(), name(63));
        QCOMPARE(results->topLevelItemCount(), 64);
        back->click();
        QCOMPARE(selectedName(), earlier);
        QVERIFY(page(1, 64, 131));
        const auto survivors = hits(name(10));
        QCOMPARE(survivors.size(), 1);
        QCOMPARE(matchData(*survivors.front())["kind"].toString(), QStringLiteral("filename"));
        next->click();
        QVERIFY(page(65, 128, 131));
        QCOMPARE(selectedName(), name(63));
        next->click();
        QVERIFY(page(129, 131, 131));
        QCOMPARE(results->topLevelItemCount(), 3);
        QCOMPARE(selectedName(), name(127));
        QVERIFY(!next->isEnabled());
        QVERIFY(selectHit(name(129)));
        control<QToolButton>("nextDocumentButton")->click();
        QCOMPARE(selectedName(), earlier);
        QVERIFY(page(1, 64, 131));
        control<QToolButton>("previousDocumentButton")->click();
        QCOMPARE(selectedName(), name(129));
        QVERIFY(page(129, 131, 131));
    }

    void performanceSettings()
    {
        auto *menu = control<QMenu>("fileMenu");
        auto *action = control<QAction>("settingsAction");
        QVERIFY(menu && action && menu->actions().contains(action));
        const auto visit = [&](QAction &open, int expectedWorkers, int expectedPdf, int newWorkers,
                               int newPdf, QDialogButtonBox::StandardButton choice, int probedLimit = 0) -> QString {
            QString error = "Settings dialog did not open";
            QObject watcher;
            QTimer::singleShot(50, &watcher, [&] {
                auto *dialog = qobject_cast<QDialog *>(QApplication::activeModalWidget());
                if (!dialog) return;
                auto *workers = dialog->findChild<QSpinBox *>("searchWorkersSpin");
                auto *pdfs = dialog->findChild<QSpinBox *>("pdfConversionsSpin");
                auto *buttons = dialog->findChild<QDialogButtonBox *>("settingsButtons");
                if (!workers || !pdfs || !buttons || !buttons->button(choice)) {
                    error = "Search workers, PDF conversions or dialog buttons missing";
                    dialog->reject();
                    return;
                }
                error.clear();
                if (probedLimit && !until([&] { return workers->maximum() == probedLimit; }, 5000))
                    error = "The worker CPU probe did not replace Qt's initial CPU estimate";
                else if (workers->value() != expectedWorkers || pdfs->value() != expectedPdf)
                    error = "Saved performance settings were not restored";
                else if (!workers->specialValueText().contains("auto", Qt::CaseInsensitive)
                         || !pdfs->specialValueText().contains("auto", Qt::CaseInsensitive)
                         || !workers->specialValueText().contains(QRegularExpression("[1-9]")))
                    error = "Automatic worker setting must show the effective count";
                else if (expectedWorkers == 0 && choice == QDialogButtonBox::Ok)
                    error = guiqa::saveView(*dialog, "performance-settings");
                workers->setValue(newWorkers);
                pdfs->setValue(newPdf);
                buttons->button(choice)->click();
            });
            open.trigger();
            return error;
        };
        guiqa::SearchStream active;
        const auto progress = frame({{"type", "progress"}, {"files", 1}});
        QVERIFY(active.prepare(progress, searchDone(0, 1)));
        control<QLineEdit>("folderEdit")->setText(active.folder.path());
        query->setText("Match");
        active.start(*search);
        QVERIFY(until([&] { return !active.request().isEmpty(); }));
        QCOMPARE(active.request().value("searchWorkers").toInt(-1), 0);
        QCOMPARE(active.request().value("pdfConversions").toInt(-1), 0);
        VERIFY_OK(visit(*action, 0, 0, 1, 1, QDialogButtonBox::Ok));
        QVERIFY(cancel->isEnabled());
        QVERIFY(active.resume());
        QVERIFY(until([&] { return search->isEnabled(); }));

        guiqa::SearchStream next;
        QVERIFY(next.prepare(progress, searchDone(0, 1)));
        control<QLineEdit>("folderEdit")->setText(next.folder.path());
        next.start(*search);
        QVERIFY(until([&] { return !next.request().isEmpty(); }));
        QCOMPARE(next.request().value("searchWorkers").toInt(-1), 1);
        QCOMPARE(next.request().value("pdfConversions").toInt(-1), 1);
        QVERIFY(next.resume());
        QVERIFY(until([&] { return search->isEnabled(); }));

        hn::MainWindow reopened;
        reopened.show();
        QVERIFY(QTest::qWaitForWindowExposed(&reopened));
        auto *storedAction = reopened.findChild<QAction *>("settingsAction");
        QVERIFY(storedAction);
        VERIFY_OK(visit(*storedAction, 1, 1, 0, 0, QDialogButtonBox::Cancel));
        VERIFY_OK(visit(*storedAction, 1, 1, 0, 0, QDialogButtonBox::Ok));
        QCOMPARE(QSettings().value("performance/searchWorkers", 0).toInt(), 0);
        QCOMPARE(QSettings().value("performance/pdfConversions", 0).toInt(), 0);

        // The Rust worker can report a higher allowance than the GUI's initial estimate.
        const int estimate = qMax(1, QThread::idealThreadCount());
        const int savedWorkers = estimate + 2, savedPdf = estimate + 1, probed = estimate + 4;
        QSettings().setValue("performance/searchWorkers", savedWorkers);
        QSettings().setValue("performance/pdfConversions", savedPdf);
        const auto installed = qgetenv("HELLONEEDLE_WORKER");
        qputenv("HELLONEEDLE_WORKER", QCoreApplication::applicationFilePath().toUtf8());
        qputenv("HN_QA_PROBE_CPUS", QByteArray::number(probed));
        const auto restored = visit(*storedAction, savedWorkers, savedPdf, savedWorkers, savedPdf,
                                    QDialogButtonBox::Ok, probed);
        qputenv("HELLONEEDLE_WORKER", installed);
        qunsetenv("HN_QA_PROBE_CPUS");
        VERIFY_OK(restored);
        QCOMPARE(QSettings().value("performance/searchWorkers").toInt(), savedWorkers);
        QCOMPARE(QSettings().value("performance/pdfConversions").toInt(), savedPdf);
    }

    void optionsAndCancellation()
    {
        ensureOptions();
        auto *pattern = control<QLineEdit>("pathPatternEdit");
        auto *reg = control<QCheckBox>("regexCheck");
        auto *sensitive = control<QCheckBox>("caseCheck");
        auto *hidden = control<QCheckBox>("hiddenCheck");
        auto *ignored = control<QCheckBox>("ignoredCheck");
        auto *names = control<QCheckBox>("filenamesCheck");
        QVERIFY(reg && sensitive && hidden && ignored && names);
        pattern->setText("*.txt");
        sensitive->setChecked(true);
        QVERIFY(submit("nEeDle"));
        QCOMPARE(results->topLevelItemCount(), 0);
        sensitive->setChecked(false);
        reg->setChecked(true);
        QVERIFY(submit("N[e]+dle"));
        QVERIFY(!hits("notes.txt").isEmpty());
        QVERIFY(hits("letter.odt").isEmpty());
        QVERIFY(submit("["));
        QCOMPARE(results->topLevelItemCount(), 0);
        reg->setChecked(false);
        pattern->clear();
        QVERIFY(submit("HiddenFixture391"));
        QVERIFY(hits(".private.txt").isEmpty());
        hidden->setChecked(true);
        QVERIFY(submit("HiddenFixture391"));
        QCOMPARE(hits(".private.txt").size(), 1);
        QVERIFY(submit("IgnoredFixture924"));
        QVERIFY(hits("ignored.txt").isEmpty());
        ignored->setChecked(true);
        QVERIFY(submit("IgnoredFixture924"));
        QCOMPARE(hits("ignored.txt").size(), 1);
        names->setChecked(true);
        QVERIFY(submit("report.pdf"));
        QVERIFY(!hits("report.pdf").isEmpty());
        names->setChecked(false);
        query->setText("Needle");
        QTest::mouseClick(search, Qt::LeftButton);
        QVERIFY(cancel->isEnabled());
        QTest::mouseClick(cancel, Qt::LeftButton);
        QVERIFY(until([&] { return !cancel->isEnabled(); }));
        QVERIFY(submit("NeverPresentAfterCancel486"));
        QTest::qWait(120);
        QCOMPARE(results->topLevelItemCount(), 0);
    }

    void languageMenu()
    {
        auto *menu = control<QMenu>("languageMenu");
        auto *en = control<QAction>("englishAction");
        auto *es = control<QAction>("spanishAction");
        QVERIFY(menu && en && es);
        QCOMPARE(menu->actions().size(), 2);
        QCOMPARE(en->text(), "English");
        QCOMPARE(es->text(), QString::fromUtf8("Español"));
        const bool beganInSpanish = es->isChecked();
        es->trigger();
        QCOMPARE(search->text(), "Buscar");
        QCOMPARE(control<QComboBox>("modeCombo")->currentText(), "Documento");
        QVERIFY(es->isChecked() && !en->isChecked());
        QCOMPARE(QSettings().value("interface/language").toString(), "es");
        en->trigger();
        QCOMPARE(search->text(), "Search");
        QCOMPARE(control<QComboBox>("modeCombo")->currentText(), "Document");
        QCOMPARE(QSettings().value("interface/language").toString(), "en");
        QVERIFY(en->isChecked() && !es->isChecked());
        if (beganInSpanish)
            es->trigger();
    }

    void helpAndCredits()
    {
        VERIFY_OK(captureModal(control<QAction>("aboutAction"), "about"));
        QVERIFY(!QCoreApplication::applicationVersion().isEmpty());
        QVERIFY(modalText.contains("Hello Needle " + QCoreApplication::applicationVersion()));
        QVERIFY(modalText.contains("Alfredo Llaquet-Alsina"));
        QVERIFY(modalText.contains("ChatGPT"));
        VERIFY_OK(captureModal(control<QAction>("licensesAction"), "licenses"));
        QVERIFY2(modalText.contains("GNU", Qt::CaseInsensitive) || modalText.contains("GPL", Qt::CaseInsensitive),
                 qPrintable(modalText.left(250)));
    }

    void nativeDirectoryPicker()
    {
        const QString previous = control<QLineEdit>("folderEdit")->text();
        auto probe = std::make_shared<NativePickerProbe>();
#if defined(Q_OS_MACOS) || defined(Q_OS_WIN)
        dismissNativeDirectoryPicker(probe);
#else
        QTimer watch;
        connect(&watch, &QTimer::timeout, this, [&] {
            auto *d = qobject_cast<QFileDialog *>(QApplication::activeModalWidget());
            if (d && d->isVisible()) {
                probe->seen = true;
                guiqa::saveView(*d, "directory-picker");
                d->reject();
                watch.stop();
            }
        });
        watch.start(60);
#endif
        control<QAbstractButton>("browseButton")->click();
        probe->finished = true;
        QVERIFY2(probe->seen, "Native directory chooser was not observed or dismissed");
        QCOMPARE(control<QLineEdit>("folderEdit")->text(), previous);
        evidence["directoryPicker"] = QGuiApplication::platformName();
    }

    void directorySelection()
    {
        const QString newPath = QDir(folder).absolutePath();
        bool observed = false;
        QTimer watch;
        connect(&watch, &QTimer::timeout, this, [&] {
            auto *d = qobject_cast<QFileDialog *>(QApplication::activeModalWidget());
            if (!d || !d->isVisible())
                return;
            watch.stop();
            observed = true;
            d->setDirectory(newPath);
            guiqa::saveView(*d, "directory-selection");
            QMetaObject::invokeMethod(d, "accept", Qt::QueuedConnection);
        });
        watch.start(70);
        control<QAbstractButton>("browseButton")->click();
        QVERIFY(observed);
        QCOMPARE(QDir(control<QLineEdit>("folderEdit")->text()).absolutePath(), newPath);
    }
};

int main(int argc, char **argv)
{
    if (argc == 2 && QByteArray(argv[1]) == "search")
        return guiqa::replaySearchStream();
    if (argc == 2 && QByteArray(argv[1]) == "--parallelism") {
        std::puts(qgetenv("HN_QA_PROBE_CPUS").constData());
        return 0;
    }
    if (qEnvironmentVariableIsSet("HN_QA_WIDGET_PICKER"))
        QApplication::setAttribute(Qt::AA_DontUseNativeDialogs);
    QApplication app(argc, argv);
    app.setQuitOnLastWindowClosed(false);
    if (app.arguments().value(1) == "--make-review")
        return guiqa::makeReviewSheet(app.arguments().value(2)) ? 0 : 1;
    QSettings::setDefaultFormat(QSettings::IniFormat);
    QSettings::setPath(QSettings::IniFormat, QSettings::UserScope, qEnvironmentVariable("HN_QA_SETTINGS"));
    hn::initializeApplication(app);
    const QString locale = qEnvironmentVariable("HN_QA_LANGUAGE", "system");
    if (locale != "system")
        hn::setLanguage(locale == "es" ? "es-MX" : "en");
    appearanceSource = guiqa::setAppearance(app, qEnvironmentVariable("HN_QA_APPEARANCE", "system"));
    GuiTests tests;
    return QTest::qExec(&tests, argc, argv);
}
#include "gui_tests.moc"
