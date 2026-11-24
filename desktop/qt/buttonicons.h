// SPDX-License-Identifier: GPL-3.0-or-later
#pragma once

class QAbstractButton;

namespace hn {
enum class ButtonIcon { Browse, Search, Cancel, Options, Details, OpenOriginal };
void setButtonIcon(QAbstractButton *button, ButtonIcon icon);
}
