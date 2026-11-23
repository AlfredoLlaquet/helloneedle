// SPDX-License-Identifier: GPL-3.0-or-later
#include <QtGlobal>
#include "picker_native.h"

#include <windows.h>
#include <chrono>
#include <cwchar>
#include <thread>

namespace {
BOOL CALLBACK dismissShellDialog(HWND window, LPARAM data)
{
    DWORD pid = 0;
    GetWindowThreadProcessId(window, &pid);
    if (pid != GetCurrentProcessId() || !IsWindowVisible(window))
        return TRUE;
    wchar_t type[64] = {};
    GetClassNameW(window, type, 64);
    if (std::wcscmp(type, L"#32770") != 0)
        return TRUE;
    auto &probe = *reinterpret_cast<NativePickerProbe *>(data);
    probe.seen = true;
    PostMessageW(window, WM_COMMAND, MAKEWPARAM(IDCANCEL, BN_CLICKED), 0);
    return FALSE;
}
}

void dismissNativeDirectoryPicker(const std::shared_ptr<NativePickerProbe> &probe)
{
    // IFileDialog can run its own modal message loop. Observe and dismiss the
    // actual visible Shell window even if Qt's event dispatcher is suspended.
    std::thread([probe] {
        for (int i = 0; i < 120 && !probe->finished; ++i) {
            EnumWindows(dismissShellDialog, reinterpret_cast<LPARAM>(probe.get()));
            std::this_thread::sleep_for(std::chrono::milliseconds(100));
        }
    }).detach();
}
