// SPDX-License-Identifier: GPL-3.0-or-later
#include "searchstatus.h"
#include "buttonicons.h"
#include "language.h"
#include <QDesktopServices>
#include <QGridLayout>
#include <QKeyEvent>
#include <QLabel>
#include <QPainter>
#include <QPushButton>
#include <QStatusBar>
#include <QStyle>
#include <QStyleOptionFocusRect>
#include <QUrl>

namespace {
// Use the status bar's foreground and plain text metrics even on styles that
// give a flat native button a bezel or a hover background.
class CrumbsLink final : public QPushButton {
public:
    explicit CrumbsLink(QWidget *parent) : QPushButton(parent)
    {
        setFlat(true);
        setAutoDefault(false);
        setFocusPolicy(Qt::StrongFocus);
        setCursor(Qt::PointingHandCursor);
        QSizePolicy policy(QSizePolicy::Maximum, QSizePolicy::Preferred);
        policy.setHeightForWidth(true);
        setSizePolicy(policy);
        connect(this, &QPushButton::clicked, this, [] {
            QDesktopServices::openUrl(QUrl(QStringLiteral("https://www.thecrumbs.org")));
        });
    }
    QSize sizeHint() const override
    {
        return fontMetrics().size(Qt::TextSingleLine, text()) + QSize(4, 4);
    }
    QSize minimumSizeHint() const override
    {
        return {fontMetrics().horizontalAdvance(QStringLiteral("TheCrumbs.org")) + 4,
                fontMetrics().height() + 4};
    }
    int heightForWidth(int width) const override
    {
        if (width >= sizeHint().width()) return sizeHint().height();
        return fontMetrics().boundingRect(0, 0, qMax(1, width - 4), 0,
                                          Qt::TextWordWrap, text()).height() + 4;
    }
protected:
    bool event(QEvent *event) override
    {
        if (event->type() == QEvent::Enter || event->type() == QEvent::Leave) {
            auto lettering = font();
            lettering.setUnderline(event->type() == QEvent::Enter);
            setFont(lettering);
        }
        return QPushButton::event(event);
    }
    void keyPressEvent(QKeyEvent *event) override
    {
        if (event->key() == Qt::Key_Return || event->key() == Qt::Key_Enter) {
            click();
            event->accept();
        } else {
            QPushButton::keyPressEvent(event);
        }
    }
    void paintEvent(QPaintEvent *) override
    {
        QPainter painter(this);
        painter.setPen(palette().color(QPalette::WindowText));
        const int lines = width() >= sizeHint().width() ? Qt::TextSingleLine : Qt::TextWordWrap;
        painter.drawText(rect().adjusted(2, 2, -2, -2), Qt::AlignCenter | lines, text());
        if (hasFocus() && testAttribute(Qt::WA_KeyboardFocusChange)) {
            QStyleOptionFocusRect frame;
            frame.initFrom(this);
            frame.rect = rect();
            style()->drawPrimitive(QStyle::PE_FrameFocusRect, &frame, &painter, this);
        }
    }
};
}

namespace hn {
SearchStatus::SearchStatus(QStatusBar *parent) : QWidget(parent), bar(parent)
{
    grid = new QGridLayout(this);
    grid->setContentsMargins(0, 0, 0, 0);
    grid->setHorizontalSpacing(6);
    grid->setVerticalSpacing(1);
    grid->setColumnStretch(0, 1);
    grid->setColumnStretch(2, 1);
    status = new QLabel(this);
    status->setObjectName("statusLabel");
    status->setTextFormat(Qt::PlainText);
    status->setSizePolicy(QSizePolicy::Ignored, QSizePolicy::Preferred);
    grid->addWidget(status, 0, 0);
    link = new CrumbsLink(this);
    link->setObjectName("crumbsLink");
    grid->addWidget(link, 0, 1, Qt::AlignCenter);
    errorGroup = new QWidget(this);
    auto *buttons = new QHBoxLayout(errorGroup);
    buttons->setContentsMargins(0, 0, 0, 0);
    errorCount = new QLabel(errorGroup);
    errorCount->setObjectName("errorCountLabel");
    buttons->addWidget(errorCount);
    details = new QPushButton(errorGroup);
    details->setObjectName("issuesButton");
    setButtonIcon(details, ButtonIcon::Details);
    buttons->addWidget(details);
    grid->addWidget(errorGroup, 0, 2, Qt::AlignRight | Qt::AlignVCenter);
    errorCount->hide();
    details->hide();
}

void SearchStatus::retranslate()
{
    link->setText(ui("Read Alfredo Llaquet-Alsina's Crumbs at TheCrumbs.org"));
    link->setToolTip(ui("Open TheCrumbs.org in your browser"));
    link->setAccessibleDescription(link->toolTip());
    details->setText(ui("Details…"));
    arrange();
}

void SearchStatus::setMessage(const QString &text, int errors)
{
    status->setText(text);
    status->setToolTip(text);
    status->setAccessibleName(text);
    errorCount->setText(errors == 1 ? ui("1 error") : ui("%1 errors").arg(errors));
    errorCount->setVisible(errors > 0);
    details->setVisible(errors > 0);
    arrange();
}

void SearchStatus::resizeEvent(QResizeEvent *event)
{
    QWidget::resizeEvent(event);
    arrange();
}

void SearchStatus::arrange()
{
    // QStatusBar may reserve a platform-specific amount of space for its size
    // grip. Balance the two margins so the text centers on the window itself.
    const int leftInset = mapTo(bar, QPoint()).x();
    const int rightInset = bar->width() - (leftInset + width());
    const int difference = rightInset - leftInset;
    const QMargins margins(qMax(0, difference), 0, qMax(0, -difference), 0);
    grid->setContentsMargins(margins);
    const int sideWidth = qMax(status->fontMetrics().horizontalAdvance(status->text()),
                               errorGroup->sizeHint().width());
    const int wideWidth = link->sizeHint().width() + 2 * (sideWidth + grid->horizontalSpacing());
    const bool stack = width() - margins.left() - margins.right() < wideWidth;
    if (compact != stack) {
        compact = stack;
        grid->removeWidget(status);
        grid->removeWidget(link);
        grid->setColumnStretch(2, compact ? 0 : 1);
        status->setWordWrap(compact);
        grid->addWidget(status, 0, 0, 1, compact ? 2 : 1);
        if (compact)
            grid->addWidget(link, 1, 0, 1, 3, Qt::AlignCenter);
        else
            grid->addWidget(link, 0, 1, Qt::AlignCenter);
    }
    // QStatusBar sizes its permanent widgets by their minimum height. Include
    // any wrapped text in that height after a resize or a counter update.
    setMinimumHeight(qMax(0, grid->heightForWidth(width())));
}
}
