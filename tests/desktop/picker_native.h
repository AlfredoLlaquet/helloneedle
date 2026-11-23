// SPDX-License-Identifier: GPL-3.0-or-later
#pragma once

#include <atomic>
#include <memory>

struct NativePickerProbe {
    std::atomic<bool> seen{false};
    std::atomic<bool> finished{false};
};

#if defined(Q_OS_WIN) || defined(Q_OS_MACOS)
void dismissNativeDirectoryPicker(const std::shared_ptr<NativePickerProbe> &probe);
#endif
