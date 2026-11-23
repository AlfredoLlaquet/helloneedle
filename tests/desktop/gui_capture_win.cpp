// SPDX-License-Identifier: GPL-3.0-or-later
#include "gui_capture.h"
#include <windows.h>

namespace guiqa {
WindowFrame printWindowsFrame(WId id)
{
    const HWND hwnd = reinterpret_cast<HWND>(id);
    DWORD pid = 0;
    GetWindowThreadProcessId(hwnd, &pid);
    RECT frame{}, client{};
    POINT clientOrigin{};
    wchar_t title[128] = {};
    if (pid != GetCurrentProcessId() || !IsWindowVisible(hwnd) ||
        !GetWindowTextW(hwnd, title, int(sizeof(title)/sizeof(*title))) ||
        QString::fromWCharArray(title) != QStringLiteral("Hello Needle") ||
        !GetWindowRect(hwnd,&frame) || !GetClientRect(hwnd,&client) ||
        !ClientToScreen(hwnd,&clientOrigin))
        return {};
    const int width = frame.right - frame.left, height = frame.bottom - frame.top;
    if (width < 100 || height < 80 || width > 10000 || height > 10000)
        return {};
    // Qt sets the process DPI awareness. GetWindowRect, ClientToScreen, the DIB
    // and PrintWindow therefore describe this same window in physical pixels.
    BITMAPINFO info{};
    info.bmiHeader.biSize = sizeof(BITMAPINFOHEADER);
    info.bmiHeader.biWidth = width;
    info.bmiHeader.biHeight = -height;
    info.bmiHeader.biPlanes = 1;
    info.bmiHeader.biBitCount = 32;
    info.bmiHeader.biCompression = BI_RGB;
    HDC display = GetDC(hwnd);
    if (!display)
        return {};
    HDC dc = CreateCompatibleDC(display);
    void *bits = nullptr;
    HBITMAP bitmap = dc ? CreateDIBSection(display, &info, DIB_RGB_COLORS, &bits, nullptr, 0) : nullptr;
    ReleaseDC(hwnd, display);
    QImage pixels;
    if (bitmap && bits) {
        HGDIOBJ previous = SelectObject(dc, bitmap);
        if (previous && previous != HGDI_ERROR) {
            // Some compositors report success but produce a blank window.
            // The caller independently checks that the client really matches.
            constexpr UINT fullContents = 0x00000002; // PW_RENDERFULLCONTENT since Windows 8.1
            if (PrintWindow(hwnd, dc, fullContents)) {
                GdiFlush();
                pixels = QImage(static_cast<const uchar *>(bits),width,height,width*4,QImage::Format_RGB32).copy();
            }
            SelectObject(dc,previous);
        }
    }
    if (bitmap) DeleteObject(bitmap);
    if (dc) DeleteDC(dc);
    return {pixels, QRect(clientOrigin.x-frame.left, clientOrigin.y-frame.top,
                          client.right-client.left, client.bottom-client.top)};
}
}
