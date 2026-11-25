// SPDX-License-Identifier: GPL-3.0-or-later
#include "performancesettings.h"
#include "language.h"
#include "runtime.h"

#include <QDialog>
#include <QDialogButtonBox>
#include <QFormLayout>
#include <QGroupBox>
#include <QJsonObject>
#include <QLabel>
#include <QProcess>
#include <QSettings>
#include <QSpinBox>
#include <QThread>
#include <QVBoxLayout>

namespace hn {
namespace {
constexpr auto workerKey = "performance/searchWorkers";
constexpr auto pdfKey = "performance/pdfConversions";

int savedCount(const QSettings &settings, const char *key)
{
    return qMax(0, settings.value(key, 0).toInt());
}

QString automaticWorkers(int count)
{
    return count == 1 ? ui("Automatic (1 worker)") : ui("Automatic (%1 workers)").arg(count);
}
}

void applyPerformanceSettings(QJsonObject &request)
{
    // A running search owns its original request. Edits take effect on the next one.
    QSettings settings;
    request.insert("searchWorkers", savedCount(settings, workerKey));
    request.insert("pdfConversions", savedCount(settings, pdfKey));
}

void showPerformanceSettings(QWidget *parent)
{
    QDialog dialog(parent);
    dialog.setObjectName("performanceSettingsDialog");
    dialog.setWindowTitle(ui("Settings"));
    auto *layout = new QVBoxLayout(&dialog);
    auto *performance = new QGroupBox(ui("Performance"), &dialog);
    auto *form = new QFormLayout(performance);
    form->setRowWrapPolicy(QFormLayout::WrapLongRows);
    layout->addWidget(performance);
    auto *workers = new QSpinBox(performance);
    workers->setObjectName("searchWorkersSpin");
    workers->setKeyboardTracking(false);
    form->addRow(ui("Search workers"), workers);

    auto *advanced = new QGroupBox(ui("Advanced"), &dialog);
    auto *pdfForm = new QFormLayout(advanced);
    pdfForm->setRowWrapPolicy(QFormLayout::WrapLongRows);
    layout->addWidget(advanced);
    auto *pdfs = new QSpinBox(advanced);
    pdfs->setObjectName("pdfConversionsSpin");
    pdfs->setKeyboardTracking(false);
    pdfForm->addRow(ui("Simultaneous PDF conversions"), pdfs);
    auto *explanation = new QLabel(ui("Each PDF conversion uses one search worker."), advanced);
    explanation->setWordWrap(true);
    pdfForm->addRow(explanation);
    auto *scope = new QLabel(ui("Changes apply to the next search."), &dialog);
    layout->addWidget(scope);
    auto *buttons = new QDialogButtonBox(QDialogButtonBox::Ok | QDialogButtonBox::Cancel, &dialog);
    buttons->setObjectName("settingsButtons");
    layout->addWidget(buttons);
    QObject::connect(buttons, &QDialogButtonBox::accepted, &dialog, &QDialog::accept);
    QObject::connect(buttons, &QDialogButtonBox::rejected, &dialog, &QDialog::reject);

    QSettings settings;
    int available = qMax(1, QThread::idealThreadCount());
    const int savedWorkers = savedCount(settings, workerKey);
    const int savedPdfs = savedCount(settings, pdfKey);
    auto updatePdfLimit = [&] {
        const int effective = workers->value() ? workers->value() : qMax(1, available / 2);
        pdfs->setRange(0, effective);
        pdfs->setSpecialValueText(ui("Automatic (%1)").arg(qMin(2, effective)));
    };
    auto setAvailable = [&](int processors) {
        available = processors;
        workers->setRange(0, available);
        workers->setSpecialValueText(automaticWorkers(qMax(1, available / 2)));
        updatePdfLimit();
    };
    // Keep a previously valid manual choice while the asynchronous probe runs.
    // Qt and the search process can see different processor allowances.
    workers->setRange(0, qMax(available, savedWorkers));
    workers->setSpecialValueText(automaticWorkers(qMax(1, available / 2)));
    workers->setValue(savedWorkers);
    updatePdfLimit();
    pdfs->setMaximum(qMax(pdfs->maximum(), savedPdfs));
    pdfs->setValue(savedPdfs);
    QObject::connect(workers, &QSpinBox::valueChanged, &dialog, updatePdfLimit);

    // The worker knows its own CPU allowance, including affinity and CPU quotas.
    // The initial Qt estimate lets the dialog open immediately on every platform.
    QProcess probe;
    QObject::connect(&probe, &QProcess::finished, &dialog, [&](int code, QProcess::ExitStatus status) {
        bool ok = false;
        const int cpus = probe.readAllStandardOutput().trimmed().toInt(&ok);
        if (status == QProcess::NormalExit && code == 0 && ok && cpus > 0)
            setAvailable(cpus);
    });
    const QString program = findTool(QStringLiteral("helloneedle-worker"));
    if (!program.isEmpty()) probe.start(program, {QStringLiteral("--parallelism")});
    if (dialog.exec() == QDialog::Accepted) {
        settings.setValue(workerKey, workers->value());
        settings.setValue(pdfKey, pdfs->value());
    }
    if (probe.state() != QProcess::NotRunning) {
        probe.kill();
        probe.waitForFinished(100);
    }
}
}
