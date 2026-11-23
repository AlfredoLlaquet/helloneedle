// SPDX-License-Identifier: GPL-3.0-or-later
#include "gui_capture.h"

#include <QDir>
#include <QScreen>
#include <QWidget>
#include <algorithm>
#include <cmath>

namespace guiqa {
namespace {
int colorGap(QRgb a, QRgb b)
{
    return std::max({std::abs(qRed(a) - qRed(b)), std::abs(qGreen(a) - qGreen(b)),
                     std::abs(qBlue(a) - qBlue(b))});
}

int neighborEdge(const QImage &image, int x, int y)
{
    return std::max(colorGap(image.pixel(x, y), image.pixel(x+1, y)),
                    colorGap(image.pixel(x, y), image.pixel(x, y+1)));
}

#ifndef Q_OS_WIN
WindowFrame compositorFrame(QWidget &window)
{
    const auto logical = window.frameGeometry();
    auto *screen = window.screen();
    const QPoint origin = logical.topLeft() - screen->geometry().topLeft();
    const QPixmap capture = screen->grabWindow(0, origin.x(), origin.y(), logical.width(), logical.height());
    if (capture.isNull() || logical.isEmpty())
        return {};
    const qreal scaleX = qreal(capture.width())/logical.width();
    const qreal scaleY = qreal(capture.height())/logical.height();
    const auto position = window.mapToGlobal(QPoint(0, 0)) - logical.topLeft();
    return {capture.toImage(), QRect(qRound(position.x()*scaleX), qRound(position.y()*scaleY),
                                     qRound(window.width()*scaleX), qRound(window.height()*scaleY))};
}
#endif
}

bool matchesWindowClient(const QImage &candidate, const QRect &area,
                         const QImage &clientReference, QJsonObject &metrics)
{
    if (candidate.isNull() || clientReference.isNull() || area.width() < 100 || area.height() < 80 ||
        !candidate.rect().adjusted(-2, -2, 2, 2).contains(area))
        return false;
    // Inputs use physical pixels. The report is normalized for Retina/Windows DPI;
    // an image from a differently shaped window cannot be stretched into a match.
    const double aspect = double(area.width())/area.height();
    const double nativeAspect = double(clientReference.width())/clientReference.height();
    if (std::abs(aspect/nativeAspect - 1.0) > 0.02)
        return false;
    const int w = 300, h = std::clamp(qRound(w/nativeAspect), 96, 480);
    const auto normalize = [=](const QImage &source) {
        return source.scaled(w,h,Qt::IgnoreAspectRatio,Qt::SmoothTransformation).convertToFormat(QImage::Format_RGB32);
    };
    const auto control = normalize(clientReference), actual = normalize(candidate.copy(area.intersected(candidate.rect())));
    int samples = 0, nearColor = 0, edges = 0, sameEdges = 0;
    int totalDelta = 0, usefulTiles = 0, matchingTiles = 0;
    // Give labels, field outlines and pane boundaries their own vote. Two unrelated
    // white windows can have similar average colors, but different edge locations.
    for (int row = 0; row < 6; ++row) {
        for (int col = 0; col < 8; ++col) {
            int tileEdges = 0, tileMatches = 0;
            const int left = std::max(2, w*col/8), right = std::min(w-3,w*(col+1)/8);
            const int top = std::max(2,h*row/6), bottom = std::min(h-3,h*(row+1)/6);
            for (int y = top; y < bottom; ++y) {
                for (int x = left; x < right; ++x) {
                    const auto ref = control.pixel(x,y);
                    const int delta = colorGap(ref, actual.pixel(x,y));
                    ++samples;
                    totalDelta += delta;
                    nearColor += delta <= 28;
                    if (neighborEdge(control,x,y) < 16)
                        continue;
                    ++tileEdges;
                    bool found = false;
                    for (int dy = -1; dy <= 1 && !found; ++dy)
                        for (int dx = -1; dx <= 1 && !found; ++dx)
                            found = colorGap(ref, actual.pixel(x+dx,y+dy)) <= 40 &&
                                    neighborEdge(actual,x+dx,y+dy) >= 12;
                    tileMatches += found;
                }
            }
            edges += tileEdges;
            sameEdges += tileMatches;
            if (tileEdges >= 5) {
                ++usefulTiles;
                matchingTiles += tileMatches >= tileEdges*.70;
            }
        }
    }
    const double colors = double(nearColor)/std::max(1,samples);
    const double structure = double(sameEdges)/std::max(1,edges);
    const double meanDelta = double(totalDelta)/std::max(1,samples);
    metrics = {{"nearColorFraction",colors}, {"meanColorDifference",meanDelta},
               {"edgeFraction",structure}, {"referenceEdgePixels",edges},
               {"featureTiles",usefulTiles},{"matchingFeatureTiles",matchingTiles}};
    return samples > 0 && edges >= 50 && usefulTiles >= 4 && colors >= .94 &&
           meanDelta <= 12 && structure >= .83 && matchingTiles >= usefulTiles*.80;
}

bool hasWindowsCaption(const QImage &frame, const QRect &client, QJsonObject &metrics)
{
    // PrintWindow may return a correct client with an unpainted DWM caption.
    // Require glyph/shape edges on both sides of the title band, staying out of
    // its border. A flat colored band does not document a native window frame.
    const int height = client.top();
    if (frame.isNull() || height < 12 || client.width() < 160 || height > frame.height()/3)
        return false;
    const int inset = std::max(2, height/7);
    const int left = std::max(0, client.left()), right = std::min(frame.width(), client.right()+1);
    const QImage pixels = frame.convertToFormat(QImage::Format_RGB32);
    auto featureRows = [&](int x1, int x2) {
        int rows = 0;
        for (int y = inset; y < height-inset; ++y) {
            int transitions = 0;
            for (int x = x1; x+1 < x2; ++x)
                transitions += colorGap(pixels.pixel(x,y), pixels.pixel(x+1,y)) >= 24;
            rows += transitions >= 3;
        }
        return rows;
    };
    const int textRows = featureRows(left + inset, left + (right-left)*2/3);
    const int controlRows = featureRows(right - std::min((right-left)/3, height*6), right-inset);
    const int required = std::max(3, height/12);
    metrics = {{"titleRowsWithInk",textRows},{"controlRowsWithInk",controlRows},{"requiredRows",required}};
    return textRows >= required && controlRows >= required;
}

void captureFirstWindow(QWidget &window, QJsonObject &evidence)
{
    QDir out(qEnvironmentVariable("HN_QA_OUTPUT"));
    const auto ref = window.grab().toImage();
    evidence["nativeFrameCapture"] = false;
    evidence["nativeWindowCapture"] = false;
    evidence["nativeCaptureUsed"] = "first-launch.png";
    const auto native = window.screen()->grabWindow(window.winId()).toImage();
    QJsonObject clientMatch;
    const bool clientOK = matchesWindowClient(native, native.rect(), ref, clientMatch) &&
                          native.save(out.filePath("first-launch-native.png"));
    evidence["directClientComparison"] = clientMatch;
    evidence["nativeWindowCapture"] = clientOK;
    if (clientOK)
        evidence["nativeCaptureUsed"] = "first-launch-native.png";

    if (!qEnvironmentVariableIsSet("GITHUB_ACTIONS"))
        return;
#ifdef Q_OS_WIN
    // Passing the target HWND avoids photographing desktop overlays such as
    // Windows first-run notices even when they cover the application.
    const auto frame = printWindowsFrame(window.winId());
    evidence["nativeFrameMethod"] = "PrintWindow(PW_RENDERFULLCONTENT)";
#else
    const auto frame = compositorFrame(window);
    evidence["nativeFrameMethod"] = "validated desktop crop";
#endif
    QJsonObject frameMatch;
    bool frameOK = matchesWindowClient(frame.image, frame.clientPixels,
                                       clientOK ? native : ref, frameMatch);
#ifdef Q_OS_WIN
    QJsonObject caption;
    frameOK = frameOK && hasWindowsCaption(frame.image, frame.clientPixels, caption);
    evidence["frameCaptionComparison"] = caption;
#endif
    frameOK = frameOK && frame.image.save(out.filePath("first-launch-frame.png"));
    evidence["frameClientComparison"] = frameMatch;
    evidence["nativeFrameCapture"] = frameOK;
    if (frameOK) {
        evidence["nativeCaptureUsed"] = "first-launch-frame.png";
    } else {
        evidence["frameFallback"] = clientOK ? "verified native window client" : "Qt client capture";
        // Never store a cross-window, black, or unverified frame as evidence.
    }
}
}
