// SPDX-License-Identifier: GPL-3.0-or-later
#include "gui_appearance.h"

#include <QAbstractButton>
#include <QAbstractItemView>
#include <QApplication>
#include <QCheckBox>
#include <QDir>
#include <QFileInfo>
#include <QFile>
#include <QJsonDocument>
#include <QHash>
#include <QImage>
#include <QJsonArray>
#include <QLabel>
#include <QLineEdit>
#include <QMenu>
#include <QPainter>
#include <QPalette>
#include <QPushButton>
#include <QScreen>
#include <QStyle>
#include <QStyleHints>
#include <QStyleOptionButton>
#include <QSysInfo>
#include <QTest>
#include <QTextBrowser>
#include <QWidget>

#include <algorithm>
#include <cmath>

namespace guiqa {
namespace {
bool dark(const QPalette &p)
{
    return p.color(QPalette::Window).lightnessF() < 0.5;
}

QColor composite(const QColor &fg, const QColor &bg)
{
    const qreal alpha = fg.alphaF();
    return QColor::fromRgbF(fg.redF() * alpha + bg.redF() * (1 - alpha),
                            fg.greenF() * alpha + bg.greenF() * (1 - alpha),
                            fg.blueF() * alpha + bg.blueF() * (1 - alpha));
}

double luminance(const QColor &c)
{
    const auto channel = [](double v) {
        return v <= 0.04045 ? v / 12.92 : std::pow((v + 0.055) / 1.055, 2.4);
    };
    return 0.2126 * channel(c.redF()) + 0.7152 * channel(c.greenF()) + 0.0722 * channel(c.blueF());
}

double contrast(const QColor &fg, const QColor &bg)
{
    const double a = luminance(composite(fg, bg));
    const double b = luminance(bg);
    return (std::max(a, b) + 0.05) / (std::min(a, b) + 0.05);
}

constexpr double minimumTextContrast = 4.5;

struct GlyphInk {
    int visible = 0;
    int core = 0;
    double peak = 1;

    void sample(double ratio, int count = 1)
    {
        if (ratio > 1.25) visible += count;
        if (ratio >= minimumTextContrast) core += count;
        peak = std::max(peak, ratio);
    }

    QString report(const QString &name, QJsonObject &numbers) const
    {
        // Antialiased edges blend with the background. Requiring multiple
        // fully visible stroke pixels avoids both testing the edges and passing
        // a blank field on the strength of a single dark pixel.
        const int required = std::max(5, visible / 100);
        numbers = {{"changedGlyphPixels", visible}, {"coreGlyphPixels", core},
                   {"requiredCorePixels", required}, {"peakContrast", peak},
                   {"minimumCoreContrast", minimumTextContrast}};
        if (visible >= 10 && core >= required)
            return {};
        return QString("%1 has %2 core glyph pixels at 4.5:1 (%3 required), peak %4:1")
            .arg(name).arg(core).arg(required).arg(peak, 0, 'f', 2);
    }
};

QString compareTextPixels(const QImage &blank, const QImage &ink, const QRect &area,
                          const QString &name, QJsonObject &metrics)
{
    if (blank.isNull() || ink.isNull() || blank.size() != ink.size())
        return "Missing matching before/after captures for " + name;
    GlyphInk glyphs;
    const QRect bounds = area.intersected(ink.rect());
    for (int y = bounds.top(); y <= bounds.bottom(); ++y)
        for (int x = bounds.left(); x <= bounds.right(); ++x)
            if (blank.pixel(x, y) != ink.pixel(x, y))
                glyphs.sample(contrast(ink.pixelColor(x, y), blank.pixelColor(x, y)));
    return glyphs.report(name, metrics);
}

QJsonArray dimensions(const QRect &rect)
{
    return {rect.x(), rect.y(), rect.width(), rect.height()};
}

// Vanilla Xvfb has no desktop settings service. Exercise a second application
// palette there while keeping the native xcb plugin, style and system capture.
QPalette x11Palette(QPalette p, bool isDark)
{
    const QColor window(isDark ? "#25282b" : "#f4f4f4");
    const QColor base(isDark ? "#1c1e21" : "#ffffff");
    const QColor text(isDark ? "#f1f2f4" : "#202124");
    const QColor selected(isDark ? "#244f7e" : "#145c9b");
    p.setColor(QPalette::Window, window);
    p.setColor(QPalette::Base, base);
    p.setColor(QPalette::Button, window);
    p.setColor(QPalette::AlternateBase, isDark ? QColor("#303437") : QColor("#e8ecf0"));
    for (auto role : {QPalette::Text, QPalette::WindowText, QPalette::ButtonText})
        p.setColor(role, text);
    p.setColor(QPalette::PlaceholderText, isDark ? QColor("#aab1b7") : QColor("#666b70"));
    p.setColor(QPalette::Highlight, selected);
    p.setColor(QPalette::HighlightedText, Qt::white);
    p.setColor(QPalette::ToolTipBase, base);
    p.setColor(QPalette::ToolTipText, text);
    p.setColor(QPalette::Link, isDark ? QColor("#8ac2ff") : QColor("#164e8a"));
    for (auto role : {QPalette::Text, QPalette::WindowText, QPalette::ButtonText})
        p.setColor(QPalette::Disabled, role, isDark ? QColor("#92969a") : QColor("#797b7d"));
    return p;
}
}

bool hasExpectedPlatform()
{
    const QString actual = QGuiApplication::platformName().toLower();
#ifdef Q_OS_WIN
    return actual == "windows";
#elif defined(Q_OS_MACOS)
    return actual == "cocoa";
#else
    return actual == "xcb" || actual.startsWith("wayland");
#endif
}

QString setAppearance(QApplication &app, const QString &requested)
{
    if (requested == "system")
        return "unmodified system appearance";
    const bool wantDark = requested == "dark";
    if (dark(app.palette()) == wantDark)
        return "system appearance";
#if QT_VERSION >= QT_VERSION_CHECK(6, 8, 0)
    app.styleHints()->setColorScheme(wantDark ? Qt::ColorScheme::Dark : Qt::ColorScheme::Light);
    for (int i = 0; i < 20 && dark(app.palette()) != wantDark; ++i)
        QTest::qWait(25);
    if (dark(app.palette()) == wantDark)
        return "native Qt platform color-scheme request";
#endif
#if !defined(Q_OS_WIN) && !defined(Q_OS_MACOS)
    app.setPalette(x11Palette(app.palette(), wantDark));
    return "explicit test palette on " + QGuiApplication::platformName();
#else
    // Do not hide a production native dark-mode failure by repainting the test.
    return "native theme request was not honored";
#endif
}

QJsonObject platformDetails(const QString &source)
{
    QScreen *screen = QGuiApplication::primaryScreen();
    const QPalette p = qApp->palette();
    return {{"os", QSysInfo::prettyProductName()}, {"architecture", QSysInfo::currentCpuArchitecture()},
            {"qt", qVersion()}, {"platformPlugin", QGuiApplication::platformName()},
            {"style", qApp->style()->objectName()}, {"font", qApp->font().toString()},
            {"appearanceSource", source}, {"dark", dark(p)},
            {"windowColor", p.color(QPalette::Window).name()}, {"textColor", p.color(QPalette::Text).name()},
            {"display", screen ? dimensions(screen->geometry()) : QJsonArray{}},
            {"workArea", screen ? dimensions(screen->availableGeometry()) : QJsonArray{}},
            {"displayDevicePixelRatio", screen ? screen->devicePixelRatio() : 0.0},
            {"scaleFactor", qEnvironmentVariable("QT_SCALE_FACTOR", "1")},
            {"requestedLanguage", qEnvironmentVariable("HN_QA_LANGUAGE")},
            {"requestedAppearance", qEnvironmentVariable("HN_QA_APPEARANCE")}};
}

QString inspectLayout(QWidget &window)
{
    const QStringList names = {"folderEdit", "browseButton", "queryEdit", "searchButton",
        "cancelButton", "optionsButton", "regexCheck", "caseCheck", "hiddenCheck", "ignoredCheck",
        "filenamesCheck", "pathPatternEdit", "modeCombo", "resultsView", "previewPane"};
    for (const QString &name : names) {
        QWidget *w = window.findChild<QWidget *>(name);
        if (!w)
            return "Missing control " + name;
        if (!w->isVisibleTo(&window))
            continue;
        const QRect rect(w->mapTo(&window, QPoint(0, 0)), w->size());
        if (!window.rect().adjusted(-1, -1, 1, 1).contains(rect) || rect.isEmpty())
            return name + " is outside the window or has collapsed";
        if (auto *edit = qobject_cast<QLineEdit *>(w)) {
            if (edit->width() < edit->fontMetrics().horizontalAdvance("Search") ||
                edit->height() < edit->fontMetrics().height())
                return name + " has insufficient usable entry space";
        }
        auto *b = qobject_cast<QAbstractButton *>(w);
        if (!b)
            continue;
        QString label = b->text();
        label.remove('&');
        if (label.isEmpty()) {
            if (b->accessibleName().isEmpty() && b->toolTip().isEmpty())
                return name + " has neither a visible label nor an accessible description";
            if (b->icon().isNull() || b->icon().pixmap(b->iconSize()).isNull())
                return name + " has no drawable icon";
            continue;
        }
        // Some platform styles intentionally leave little horizontal padding.
        QStyleOptionButton opt;
        opt.initFrom(b);
        opt.text = b->text();
        QRect content = b->rect();
        if (qobject_cast<QPushButton *>(b))
            content = b->style()->subElementRect(QStyle::SE_PushButtonContents, &opt, b);
        else if (qobject_cast<QCheckBox *>(b))
            content = b->style()->subElementRect(QStyle::SE_CheckBoxContents, &opt, b);
        if (b->fontMetrics().horizontalAdvance(label) > content.width() + 2 ||
            b->fontMetrics().height() > b->height() + 1)
            return QString("%1 clips '%2' (%3 px available)").arg(name, label).arg(content.width());
    }
    return {};
}

QString inspectColors(QWidget &window, QJsonObject &colors)
{
    QString failure;
    auto measure = [&](QWidget *w, const QString &part, QPalette::ColorRole fg,
                       QPalette::ColorRole bg, double minimum) {
        if (!w) {
            failure += part + " control is missing. ";
            return;
        }
        const QPalette p = w->palette();
        const QColor a = p.color(QPalette::Active, fg), b = p.color(QPalette::Active, bg);
        const double ratio = contrast(a, b);
        colors[part] = QJsonObject{{"foreground", a.name(QColor::HexArgb)}, {"background", b.name()},
                                  {"ratio", ratio}, {"minimum", minimum}};
        if (ratio + 0.01 < minimum)
            failure += QString("%1 %2:1, expected %3:1. ").arg(part).arg(ratio, 0, 'f', 2).arg(minimum);
    };
    for (const QString &name : {QString("folderEdit"), QString("queryEdit"), QString("pathPatternEdit")}) {
        auto *w = window.findChild<QLineEdit *>(name);
        measure(w, name + "/text", QPalette::Text, QPalette::Base, 4.5);
        if (w && !w->placeholderText().isEmpty())
            measure(w, name + "/placeholder", QPalette::PlaceholderText, QPalette::Base, 4.5);
    }
    auto *tree = window.findChild<QAbstractItemView *>("resultsView");
    measure(window.findChild<QWidget *>("previewText"), "preview/text", QPalette::Text, QPalette::Base, 4.5);
    measure(window.findChild<QWidget *>("previewText"), "preview/placeholder", QPalette::PlaceholderText, QPalette::Base, minimumTextContrast);
    measure(tree, "results/text", QPalette::Text, QPalette::Base, 4.5);
    measure(tree, "results/selected", QPalette::HighlightedText, QPalette::Highlight, 3.0);
    measure(window.findChild<QMenu *>("fileMenu"), "menu/text", QPalette::WindowText, QPalette::Window, 4.5);
    measure(window.findChild<QAbstractButton *>("searchButton"), "button/label", QPalette::ButtonText, QPalette::Button, 4.5);
    return failure;
}

QString inspectMenuInk(QWidget &menu, const QRect &action, QJsonObject &metrics)
{
    const QRect inner = action.adjusted(3, 3, -3, -3);
    if (inner.isEmpty())
        return "Menu label has no visible area";
    QImage shot = menu.grab(inner).toImage().convertToFormat(QImage::Format_RGB32);
    if (shot.isNull())
        return "Menu text capture is unavailable";
    QHash<QRgb, int> histogram;
    for (int y = 0; y < shot.height(); ++y)
        for (int x = 0; x < shot.width(); ++x)
            ++histogram[shot.pixel(x, y)];
    const auto background = std::max_element(histogram.cbegin(), histogram.cend());
    if (background == histogram.cend())
        return "Menu text capture is empty";
    const QColor base = QColor::fromRgb(background.key());
    GlyphInk glyphs;
    for (auto it = histogram.cbegin(); it != histogram.cend(); ++it)
        glyphs.sample(contrast(QColor::fromRgb(it.key()), base), it.value());
    const QString failure = glyphs.report("Enabled menu label", metrics);
    metrics["background"] = base.name();
    return failure;
}

QString inspectRenderedField(QLineEdit &edit, QJsonObject &metrics)
{
    if (!edit.isEnabled() || !edit.isVisible() || edit.placeholderText().isEmpty())
        return edit.objectName() + " is missing an enabled, visible prompt";
    const QString saved = edit.text(), hint = edit.placeholderText();
    edit.clearFocus();
    edit.clear();
    edit.setPlaceholderText({});
    const auto pixels = [&] { return edit.grab().toImage().convertToFormat(QImage::Format_RGB32); };
    const QImage blank = pixels();
    QStringList failures;
    auto measure = [&](const QString &label) {
        const QImage ink = pixels();
        const qreal dpr = ink.devicePixelRatio();
        const int padding = qRound(3*dpr);
        const int inset = qRound(5*dpr);
        // Exclude the frame and the platform's clear button from the scan.
        const QRect area(inset, padding, ink.width() - qRound(26*dpr), ink.height() - 2*padding);
        QJsonObject reading;
        const QString issue = compareTextPixels(blank, ink, area, edit.objectName() + "/" + label, reading);
        metrics[label] = reading;
        if (!issue.isEmpty())
            failures.append(issue);
    };
    edit.setPlaceholderText(hint);
    measure("placeholder");
    edit.setPlaceholderText({});
    edit.setText("Needle");
    measure("enteredText");
    edit.setText(saved);
    edit.setPlaceholderText(hint);
    return failures.join(". ");
}

QString inspectPreviewPrompt(QTextBrowser &preview, QJsonObject &metrics)
{
    const auto prompt = preview.placeholderText();
    if (!preview.isEnabled() || !preview.isVisible() || prompt.isEmpty() || !preview.toPlainText().isEmpty())
        return "Enabled first-launch preview help is missing";
    auto *viewport = preview.viewport();
    const auto capture = [&] { return viewport->grab().toImage().convertToFormat(QImage::Format_RGB32); };
    preview.setPlaceholderText({});
    const QImage blank = capture();
    preview.setPlaceholderText(prompt);
    const QImage ink = capture();
    // Only the first lines of the viewport contain the initial help paragraph.
    const int height = qCeil((preview.fontMetrics().height()*3 + 12) * ink.devicePixelRatio());
    const QRect area(0, 0, ink.width(), std::min(ink.height(), height));
    return compareTextPixels(blank, ink, area, "Enabled preview help", metrics);
}

QString saveView(QWidget &widget, const QString &name)
{
    const QString directory = qEnvironmentVariable("HN_QA_OUTPUT");
    const QString path = QDir(directory).filePath(name + ".png");
    const QPixmap view = widget.grab();
    if (view.isNull() || view.width() < 20 || !view.save(path, "PNG"))
        return "Could not save " + path;
    return {};
}

bool makeReviewSheet(const QString &root)
{
    struct Panel {QString name; QString state;};
    const QList<Panel> panels = {
        {"system", "first-launch"}, {"system", "icon-grid"},
        {"en-dark-100", "first-launch"}, {"es-light-100", "options-narrow"},
        {"es-dark-150", "options-narrow"}, {"en-light-150", "search-results"},
        {"es-dark-100", "word-preview"}, {"en-dark-100", "pdf-preview"}
    };
    const int tileWidth = 565, tileHeight = 358;
    QImage sheet(tileWidth * 2, tileHeight * 4, QImage::Format_RGB32);
    sheet.fill(QColor("#eceff1"));
    QPainter painter(&sheet);
    painter.setRenderHint(QPainter::SmoothPixmapTransform);
    QFont font = painter.font();
    font.setPixelSize(15);
    painter.setFont(font);
    bool complete = true;
    for (qsizetype i = 0; i < panels.size(); ++i) {
        const auto &panel = panels.at(i);
        const QString rel = panel.name + "/" + panel.state;
        QImage source;
        if (panel.name == "system" && panel.state == "first-launch") {
            QFile data(QDir(root).filePath(panel.name + "/appearance.json"));
            if (data.open(QIODevice::ReadOnly)) {
                const auto record = QJsonDocument::fromJson(data.readAll()).object();
                const QString file = record.value("nativeCaptureUsed").toString();
                if (file == "first-launch-frame.png" || file == "first-launch-native.png")
                    source.load(QDir(root).filePath(panel.name + "/" + file));
            }
        }
        if (source.isNull())
            source.load(QDir(root).filePath(rel + ".png"));
        const int x = static_cast<int>(i % 2) * tileWidth + 8;
        const int y = static_cast<int>(i / 2) * tileHeight + 5;
        painter.setPen(QColor("#1a1e22"));
        painter.drawText(QRect(x, y, tileWidth-16, 22), Qt::AlignLeft | Qt::AlignVCenter, rel);
        if (source.isNull()) {
            complete = false;
            continue;
        }
        const QSize size = source.size().scaled(tileWidth-16, tileHeight-35, Qt::KeepAspectRatio);
        painter.drawImage(QRect(QPoint(x, y+25), size), source);
    }
    painter.end();
    return sheet.save(QDir(root).filePath("review.png")) && complete;
}
}
