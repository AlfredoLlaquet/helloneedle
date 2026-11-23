// SPDX-License-Identifier: GPL-3.0-or-later
#include <QtGlobal>
#include "picker_native.h"
#include <QElapsedTimer>
#include <QGuiApplication>
#include <QScreen>
#include <QTest>
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

QStringList cocoaMenuTitles()
{
    QStringList titles;
    for (NSMenuItem *item in NSApp.mainMenu.itemArray) {
        if (!item.hidden && item.title.length > 0)
            titles.append(QString::fromUtf8(item.title.UTF8String));
    }
    return titles;
}

QStringList cocoaMenuActions(const QString &menuTitle)
{
    for (NSMenuItem *item in NSApp.mainMenu.itemArray) {
        if (!item.hidden && QString::fromUtf8(item.title.UTF8String) == menuTitle) {
            NSMenu *submenu = item.submenu;
            [submenu update];
            QStringList actions;
            for (NSMenuItem *child in submenu.itemArray) {
                if (!child.hidden && child.enabled && child.action && child.title.length)
                    actions.append(QString::fromUtf8(child.title.UTF8String));
            }
            return actions;
        }
    }
    return {};
}

QPixmap captureActiveCocoaMenu(quintptr viewId, QJsonObject &state)
{
    NSWindow *window = reinterpret_cast<NSView *>(viewId).window;
    NSWorkspace *workspace = NSWorkspace.sharedWorkspace;
    const pid_t pid = NSProcessInfo.processInfo.processIdentifier;
    // A command-line QtTest can receive keyboard focus yet Finder may still
    // own the system menu. Request foreground status for this test process.
    [NSApp setActivationPolicy:NSApplicationActivationPolicyRegular];
    [window makeKeyAndOrderFront:nil];
    if (@available(macOS 14.0, *)) [NSApp activate];
    else [NSApp activateIgnoringOtherApps:YES];

    auto isOwner = [&] {
        return NSApp.active && workspace.frontmostApplication.processIdentifier == pid &&
               workspace.menuBarOwningApplication.processIdentifier == pid;
    };
    auto recordOwner = [&] {
        state["processPID"] = int(pid);
        state["frontmostPID"] = int(workspace.frontmostApplication.processIdentifier);
        state["menuOwnerPID"] = int(workspace.menuBarOwningApplication.processIdentifier);
        state["foregroundWindow"] = bool(window.keyWindow);
    };
    QElapsedTimer wait, stable;
    wait.start();
    while (wait.elapsed() < 2500) {
        QTest::qWait(40);
        if (!isOwner()) stable.invalidate();
        else if (!stable.isValid()) stable.start();
        else if (stable.elapsed() >= 300) break;
    }
    recordOwner();
    if (!isOwner() || !stable.isValid() || stable.elapsed() < 300) {
        state["reason"] = "AppKit did not grant stable ownership of the visible menu bar";
        return {};
    }
    // The display whose Quartz origin is (0, 0) owns the primary Apple menu.
    // A window can be on a different display; do not sample its top edge.
    NSScreen *nativeScreen = NSScreen.screens.firstObject;
    QScreen *screen = nullptr;
#if QT_VERSION >= QT_VERSION_CHECK(6, 11, 0)
    for (auto *candidate : QGuiApplication::screens()) {
        const auto *native = candidate->nativeInterface<QNativeInterface::QCocoaScreen>();
        if (native && native->nativeScreen() == nativeScreen) {
            screen = candidate;
            break;
        }
    }
#else
    screen = QGuiApplication::primaryScreen();
#endif
    if (!screen || !nativeScreen) {
        state["reason"] = "The primary menu display was unavailable";
        return {};
    }
    const int height = qBound(20, qRound(NSApp.mainMenu.menuBarHeight), 48);
    state["display"] = screen->name();
    state["height"] = height;
    const auto shot = screen->grabWindow(0, 0, 0, qMin(750, screen->size().width()), height);
    QTest::qWait(35);
    recordOwner();
    if (!isOwner() || shot.isNull()) {
        state["reason"] = "Screen pixels were unavailable or native menu ownership changed";
        return {};
    }
    return shot;
}
