// SPDX-License-Identifier: GPL-3.0-or-later
#include "application.h"
#include "language.h"
#include "mainwindow.h"
#include "gui_appearance.h"
#include "gui_icon.h"
#include "picker_native.h"

#include <QAbstractButton>
#include <QAction>
#include <QApplication>
#include <QCheckBox>
#include <QComboBox>
#include <QDir>
#include <QElapsedTimer>
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
#include <QScreen>
#include <QSettings>
#include <QSet>
#include <QStyle>
#include <QTest>
#include <QTextBlock>
#include <QTextBrowser>
#include <QTimer>
#include <QToolButton>
#include <QTreeWidget>

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

    bool submit(const QString &text, bool keyboard = false)
    {
        control<QLineEdit>("folderEdit")->setText(folder);
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
        QCOMPARE(options->arrowType(), Qt::RightArrow);
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
        const QPixmap native = guiqa::hasExpectedPlatform() ? window->screen()->grabWindow(window->winId()) : QPixmap();
        evidence["nativeWindowCapture"] = saveNative(native, "first-launch-native");
        // Capture the actual non-client frame on an isolated native runner as
        // well. Full desktop capture is unnecessary for checking initial chrome.
        if (guiqa::hasExpectedPlatform() && qEnvironmentVariableIsSet("GITHUB_ACTIONS")) {
            const QPoint origin = frame.topLeft() - window->screen()->geometry().topLeft();
            const auto framed = window->screen()->grabWindow(0, origin.x(), origin.y(),
                                                             frame.width(), frame.height());
            evidence["nativeFrameCapture"] = saveNative(framed, "first-launch-frame");
        }
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
        QCOMPARE(control<QToolButton>("optionsButton")->arrowType(), Qt::DownArrow);
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
            QVERIFY2(titles.contains(nativeTitle("searchMenu")) && titles.contains(nativeTitle("languageMenu")),
                     qPrintable("Cocoa did not publish the translated menus: " + titles.join(", ")));
            if (qEnvironmentVariableIsSet("GITHUB_ACTIONS") && qApp->applicationState() == Qt::ApplicationActive) {
                auto *screen = window->screen();
                const int barHeight = qBound(24, screen->availableGeometry().top() - screen->geometry().top(), 40);
                const auto native = screen->grabWindow(0, 0, 0, qMin(600, screen->geometry().width()), barHeight);
                evidence["cocoaMenuBarCapture"] = saveNative(native, "native-menu-bar");
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
        QVERIFY(control<QAbstractButton>("openOriginalButton")->isEnabled());
        VERIFY_OK(guiqa::saveView(*window, "search-results"));
        QVERIFY(submit("NeverPresentInFixture0204"));
        QCOMPARE(results->topLevelItemCount(), 0);
        QVERIFY(submit("Needle"));
        QCOMPARE(hits("notes.txt").size(), 2);
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
        const auto first = image->pixmap().toImage();
        QVERIFY(selectHit("report.pdf", 1));
        QVERIFY(until([&] { return ready() && image->pixmap().toImage() != first; }));
        VERIFY_OK(guiqa::saveView(*window, "pdf-preview"));
        const QSize original = image->pixmap().size();
        QTest::mouseClick(zoom, Qt::LeftButton);
        QVERIFY(until([&] { return image->pixmap().width() > original.width(); }));
        QTest::mouseClick(fit, Qt::LeftButton);
        QVERIFY(until([&] { return image->pixmap().width() <= original.width() + 2; }));
        QTest::mouseClick(prev, Qt::LeftButton);
        QVERIFY(until([&] { return !prev->isEnabled() && label->text().contains('1'); }));
        QTest::mouseClick(next, Qt::LeftButton);
        QVERIFY(until([&] { return prev->isEnabled() && label->text().contains('2'); }));
        QTest::mouseClick(next, Qt::LeftButton);
        QVERIFY(until([&] { return !next->isEnabled() && label->text().contains('3'); }));
        auto *toggle = control<QAbstractButton>("pdfTextButton");
        QVERIFY(toggle && toggle->isVisible());
        QTest::mouseClick(toggle, Qt::LeftButton);
        QVERIFY(until([&] { return preview->isVisible() && preview->textCursor().block().text().contains("Closing page"); }));
        QTest::mouseClick(toggle, Qt::LeftButton);
        QVERIFY(until([&] { return image->isVisible() && !image->pixmap().isNull() && !next->isEnabled(); }));
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
