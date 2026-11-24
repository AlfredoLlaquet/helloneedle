// SPDX-License-Identifier: GPL-3.0-or-later
#include "language.h"
#include <QApplication>
#include <QHash>
#include <QJsonObject>
#include <QLibraryInfo>
#include <QLocale>
#include <QTranslator>
#include <memory>

namespace {
// Source strings double as the fallback. The only maintained translation is
// Spanish. Qt's own dialog catalog is loaded separately when available.
QString active = QStringLiteral("en");
QTranslator *nativeSpanish = nullptr;
const QHash<QByteArray, QString> &spanish()
{
    static const QHash<QByteArray, QString> strings{
        {"%1 documents scanned", "%1 documentos examinados"}, {"1 document scanned", "1 documento examinado"},
        {"%1 matches", "%1 coincidencias"}, {"1 match", "1 coincidencia"},
        {"%1 errors", "%1 errores"}, {"1 error", "1 error"},
        {"&File", "&Archivo"}, {"&Help", "A&yuda"}, {"&Language", "&Idioma"}, {"&Search", "&Buscar"},
        {"About Hello Needle", "Acerca de Hello Needle"}, {"Additional file errors: %1", "Otros errores de archivo: %1"},
        {"Based on Clapgrep by Leopold Luley and contributors.", "Basado en Clapgrep, de Leopold Luley y sus colaboradores."},
        {"Browse…", "Examinar…"}, {"Cancel search", "Cancelar búsqueda"}, {"Cancel", "Cancelar"},
        {"Case sensitive", "Distinguir mayúsculas y minúsculas"}, {"Choose a folder", "Elegir una carpeta"},
        {"Choose a folder.", "Selecciona una carpeta."}, {"Choose a local folder", "Elige una carpeta local"},
        {"Choose folder…", "Elegir carpeta…"}, {"Close", "Cerrar"},
        {"Created by Alfredo Llaquet-Alsina.", "Creado por Alfredo Llaquet-Alsina."},
        {"Details…", "Detalles…"}, {"Document preview", "Vista previa del documento"}, {"Document", "Documento"},
        {"Enter a word or phrase.", "Introduce una palabra o frase."},
        {"Find", "Buscar"}, {"Fit", "Ajustar"}, {"Fit page", "Ajustar página"}, {"Focus search field", "Ir al campo de búsqueda"},
        {"Folder", "Carpeta"}, {"For example, *.md", "Por ejemplo, *.md"},
        {"GNU General Public License, version 3 or later.", "Licencia Pública General de GNU, versión 3 o posterior."},
        {"Include hidden files", "Incluir archivos ocultos"}, {"Include ignored files", "Incluir archivos ignorados"},
        {"License files", "Archivos de licencia"}, {"License text", "Texto de la licencia"}, {"Licenses", "Licencias"},
        {"Loading page…", "Cargando página…"}, {"Loading preview…", "Cargando vista previa…"},
        {"Next", "Siguiente"}, {"Next page", "Página siguiente"},
        {"Open TheCrumbs.org in your browser", "Abrir TheCrumbs.org en tu navegador"},
        {"Open original", "Abrir original"}, {"Open this file in its usual application.", "Abrir el archivo en su aplicación habitual."},
        {"Options", "Opciones"}, {"PDF page rendering requires Poppler (pdftoppm).", "La vista de páginas PDF requiere Poppler (pdftoppm)."},
        {"PDF page", "Página PDF"}, {"Page %1 of %2", "Página %1 de %2"}, {"Path filter", "Filtro de ruta"}, {"Preview", "Vista previa"},
        {"Previous", "Anterior"}, {"Previous page", "Página anterior"}, {"Programmed by ChatGPT.", "Programado por ChatGPT."},
        {"Quit", "Salir"}, {"Read Alfredo Llaquet-Alsina's Crumbs at TheCrumbs.org", "Lee Crumbs de Alfredo Llaquet-Alsina en TheCrumbs.org"},
        {"Ready to search.", "Todo listo para buscar."}, {"Regular expression", "Expresión regular"},
        {"Results", "Resultados"}, {"Search canceled.", "Búsqueda cancelada."}, {"Search details", "Detalles de la búsqueda"},
        {"Search file names", "Buscar en los nombres de archivo"}, {"Search in", "Buscar en"},
        {"Search inside local documents.", "Busca en el contenido de documentos locales."},
        {"Search options", "Opciones de búsqueda"}, {"Search query", "Texto de búsqueda"},
        {"Search readable text or the saved HTML and Markdown markup.", "Buscar en el texto legible o en el código HTML y Markdown guardado."},
        {"Search results", "Resultados de la búsqueda"}, {"Search", "Buscar"}, {"Searching… %1", "Buscando… %1"},
        {"Select a result to read the passage.", "Selecciona un resultado para leer el pasaje."},
        {"Show page", "Ver página"}, {"Show text", "Ver texto"}, {"Showing the first %1 matches.", "Se muestran las primeras %1 coincidencias."},
        {"Source", "Código fuente"}, {"The PDF page could not be rendered.", "No se ha podido mostrar la página del PDF."},
        {"The license text could not be opened.", "No se ha podido abrir el texto de la licencia."},
        {"The original file could not be opened.", "No se ha podido abrir el archivo original."},
        {"The search worker could not be started.", "No se ha podido iniciar el proceso de búsqueda."},
        {"The search worker stopped unexpectedly.", "El proceso de búsqueda se ha detenido de forma inesperada."},
        {"The worker response is too large.", "La respuesta del proceso de búsqueda es demasiado grande."},
        {"The worker returned an unreadable response.", "La respuesta del proceso de búsqueda no se puede leer."},
        {"The worker stopped before the search finished.", "El proceso se ha detenido antes de terminar la búsqueda."},
        {"This document contains no previewable text.", "Este documento no contiene texto que pueda mostrarse."},
        {"Word or phrase", "Palabra o frase"}, {"Zoom in", "Ampliar"}, {"Zoom out", "Reducir"},
        {"File name", "Nombre de archivo"}, {"Line %1", "Línea %1"}, {"Page %1", "Página %1"},
        {"Slide %1", "Diapositiva %1"}, {"Paragraph %1", "Párrafo %1"}, {"Heading %1", "Encabezado %1"},
        {"Table %1, row %2, column %3", "Tabla %1, fila %2, columna %3"}, {"Passage %1", "Pasaje %1"},
        {"Invalid search request.", "La solicitud de búsqueda no es válida."},
        {"Invalid search expression or path filter.", "La expresión de búsqueda o el filtro de ruta no son válidos."},
        {"The search folder is unavailable.", "La carpeta de búsqueda no está disponible."},
        {"The file could not be read.", "No se ha podido leer el archivo."},
        {"This file format or variant is unsupported.", "Este formato de archivo o variante no es compatible."},
        {"The document exceeds the reader’s limits.", "El documento supera los límites de lectura."},
        {"The selected location is no longer available.", "La ubicación seleccionada ya no está disponible."},
        {"The document has changed. Repeat the search.", "El documento ha cambiado. Repite la búsqueda."},
        {"A required document reader is unavailable.", "Falta un lector necesario para este documento."},
        {"The document is encrypted or password-protected.", "El documento está cifrado o protegido con contraseña."},
        {"The PDF has no searchable text layer.", "El PDF carece de una capa de texto que permita búsquedas."},
        {"The text encoding is unsupported.", "La codificación del texto no es compatible."},
        {"The document could not be decoded.", "No se ha podido interpretar el documento."},
        {"Select folder", "Seleccionar carpeta"}, {"Look in:", "Buscar en:"}, {"Folder:", "Carpeta:"}
    };
    return strings;
}
}

namespace hn {
void setLanguage(const QString &localeName)
{
    const QString code = localeName.trimmed().toLower().section(QLatin1Char('-'), 0, 0).section(QLatin1Char('_'),0,0).section(QLatin1Char('.'),0,0);
    active = code == QStringLiteral("es") ? QStringLiteral("es") : QStringLiteral("en");
    QLocale::setDefault(QLocale(localeName.isEmpty() || (code != "en" && code != "es") ? active : localeName));
    if (nativeSpanish && qApp) {
        qApp->removeTranslator(nativeSpanish);
        delete nativeSpanish;
        nativeSpanish = nullptr;
    }
    if (active == "es" && qApp) {
        auto *t = new QTranslator(qApp);
        if (t->load(QStringLiteral("qtbase_es"), QLibraryInfo::path(QLibraryInfo::TranslationsPath))) {
            qApp->installTranslator(t);
            nativeSpanish = t;
        } else delete t;
    }
}
QString currentLanguage() { return active; }
QString systemLanguage(const QStringList &uiLanguages)
{
    return uiLanguages.isEmpty() ? QStringLiteral("en") : uiLanguages.constFirst();
}
QString ui(const char *english)
{
    if (active == "es") {
        const auto it = spanish().constFind(english);
        if (it != spanish().cend()) return it.value();
    }
    return QString::fromUtf8(english);
}

QString locationLabel(const QJsonObject &loc, const QString &fallback)
{
    if (loc.value("filename").toBool()) return ui("File name");
    if (loc.contains("cell")) return loc.value("sheet").toString() + QStringLiteral(" · ") + loc.value("cell").toString();
    if (loc.contains("page")) return ui("Page %1").arg(loc.value("page").toInt());
    if (loc.contains("slide")) return ui("Slide %1").arg(loc.value("slide").toInt());
    if (loc.contains("table") && loc.contains("row") && loc.contains("column"))
        return ui("Table %1, row %2, column %3").arg(loc.value("table").toInt()).arg(loc.value("row").toInt()).arg(loc.value("column").toInt());
    if (loc.contains("line")) return ui("Line %1").arg(loc.value("line").toInt());
    if (loc.contains("paragraph")) return ui("Paragraph %1").arg(loc.value("paragraph").toInt());
    if (loc.value("role") == "paragraph") return ui("Paragraph %1").arg(loc.value("index").toInt() + 1);
    if (loc.value("role") == "heading") return ui("Heading %1").arg(loc.value("index").toInt() + 1);
    if (active == "en" && !fallback.isEmpty()) return fallback;
    return ui("Passage %1").arg(loc.value("index").toInt() + 1);
}

QString workerError(const QJsonObject &event)
{
    const QString code = event.value("code").toString();
    static const QHash<QString, const char *> errors{
        {"request", "Invalid search request."}, {"query", "Invalid search expression or path filter."},
        {"folder", "The search folder is unavailable."}, {"walk", "The file could not be read."},
        {"read", "The file could not be read."}, {"unreadable_document", "The file could not be read."},
        {"format", "This file format or variant is unsupported."}, {"unsupported_document", "This file format or variant is unsupported."},
        {"limit", "The document exceeds the reader’s limits."}, {"too_large", "The document exceeds the reader’s limits."},
        {"resource_limit", "The document exceeds the reader’s limits."}, {"location", "The selected location is no longer available."},
        {"changed", "The document has changed. Repeat the search."}, {"tool_missing", "A required document reader is unavailable."},
        {"missing_decoder", "A required document reader is unavailable."}, {"protected", "The document is encrypted or password-protected."},
        {"encrypted_document", "The document is encrypted or password-protected."}, {"no_text_layer", "The PDF has no searchable text layer."},
        {"unsupported_encoding", "The text encoding is unsupported."}, {"invalid_document", "The document could not be decoded."},
        {"corrupt_document", "The document could not be decoded."}
    };
    const QString details = event.value("message").toString().trimmed();
    if (active == "en" && !details.isEmpty()) return details;
    return ui(errors.value(code, "The document could not be decoded."));
}
}
