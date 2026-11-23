// SPDX-License-Identifier: GPL-3.0-or-later
#include <QtGlobal>
#include "picker_native.h"
#import <AppKit/AppKit.h>
#import <dispatch/dispatch.h>

namespace {
void waitForPanel(const std::shared_ptr<NativePickerProbe> &probe, int remaining)
{
    if (probe->finished || remaining == 0)
        return;
    for (NSWindow *window in NSApp.windows) {
        if ([window isKindOfClass:[NSOpenPanel class]] && window.visible) {
            probe->seen = true;
            [(NSOpenPanel *)window cancel:nil];
            return;
        }
    }
    auto ownedProbe = probe;
    dispatch_after(dispatch_time(DISPATCH_TIME_NOW, 100 * NSEC_PER_MSEC),
                   dispatch_get_main_queue(), ^{ waitForPanel(ownedProbe, remaining - 1); });
}
}

void dismissNativeDirectoryPicker(const std::shared_ptr<NativePickerProbe> &probe)
{
    // Let getExistingDirectory enter the AppKit run loop first.
    auto ownedProbe = probe;
    dispatch_async(dispatch_get_main_queue(), ^{ waitForPanel(ownedProbe, 120); });
}
