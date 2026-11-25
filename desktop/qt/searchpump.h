// SPDX-License-Identifier: GPL-3.0-or-later
#pragma once
#include <QSemaphore>
#include <QThread>
#include <atomic>
#include <functional>

namespace hn {
// QProcess reads an asynchronous pipe into its internal ring buffer. Drive it
// synchronously on this thread so a slower GUI eventually blocks the producer
// in the kernel pipe, even when a folder produces millions of occurrences.
class SearchPump final : public QThread {
public:
    enum class Outcome { Completed, StartFailed, WorkerFailed };
    using Delivery = std::function<void(SearchPump *, const QByteArray &)>;
    SearchPump(QString executable, QByteArray request, QObject *receiver, Delivery deliver);
    void cancel();
    Outcome outcome() const { return ending; }
protected:
    void run() override;
private:
    QString executable;
    QByteArray request;
    QObject *receiver;
    Delivery deliver;
    QSemaphore available{2};
    std::atomic_bool canceled{false};
    Outcome ending = Outcome::Completed;
};
}
