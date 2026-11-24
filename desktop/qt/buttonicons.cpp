// SPDX-License-Identifier: GPL-3.0-or-later
#include "buttonicons.h"
#include <QAbstractButton>
#include <QApplication>
#include <QFile>
#include <QIconEngine>
#include <QPainter>
#include <QPointer>
#include <QSvgRenderer>

namespace {
const char *iconName(hn::ButtonIcon icon)
{
    using hn::ButtonIcon;
    switch (icon) {
    case ButtonIcon::Browse: return "folder-open";
    case ButtonIcon::Search: return "search";
    case ButtonIcon::Cancel: return "circle-stop";
    case ButtonIcon::Options: return "sliders-horizontal";
    case ButtonIcon::Details: return "list";
    case ButtonIcon::OpenOriginal: return "external-link";
    }
    Q_UNREACHABLE();
}

class ButtonIconEngine final : public QIconEngine {
public:
    ButtonIconEngine(const QByteArray &artwork, QAbstractButton *owner) : svg(artwork), button(owner) {}
    QIconEngine *clone() const override { return new ButtonIconEngine(svg, button); }

    void paint(QPainter *painter, const QRect &rect, QIcon::Mode mode, QIcon::State) override
    {
        const QPalette palette = button ? button->palette() : QApplication::palette();
        const auto group = mode == QIcon::Disabled ? QPalette::Disabled : palette.currentColorGroup();
        const auto ink = palette.color(group, QPalette::ButtonText);
        QByteArray colored = svg;
        QSvgRenderer renderer(colored.replace("currentColor", ink.name().toLatin1()));
        painter->save();
        painter->setOpacity(painter->opacity() * ink.alphaF());
        renderer.render(painter, rect);
        painter->restore();
    }

    QPixmap pixmap(const QSize &size, QIcon::Mode mode, QIcon::State state) override
    {
        QPixmap image(size);
        image.fill(Qt::transparent);
        QPainter painter(&image);
        paint(&painter, image.rect(), mode, state);
        return image;
    }

private:
    QByteArray svg;
    QPointer<QAbstractButton> button;
};
}

void hn::setButtonIcon(QAbstractButton *button, ButtonIcon icon)
{
    QFile source(QStringLiteral(":/art/icons/%1.svg").arg(QString::fromLatin1(iconName(icon))));
    if (source.open(QIODevice::ReadOnly))
        button->setIcon(QIcon(new ButtonIconEngine(source.readAll(), button)));
}
